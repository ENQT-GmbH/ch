use futures::{future, pin_mut, Future};
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};

use super::{msg::RFnRequest, CallError};
use crate::{
    codec::CodecT,
    rsync::{mpsc, oneshot, RemoteSend},
};

/// Provides a remotely callable async Fn function.
///
/// Dropping the provider will stop making the function available for remote calls.
pub struct RFnProvider {
    keep_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl fmt::Debug for RFnProvider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("RFnProvider").finish_non_exhaustive()
    }
}

impl RFnProvider {
    /// Keeps the provider alive until it is not required anymore.
    pub fn keep(mut self) {
        let _ = self.keep_tx.take().unwrap().send(());
    }

    /// Waits until the provider can be safely dropped.
    ///
    /// This is the case when the [RFn] is dropped.
    pub async fn done(&mut self) {
        self.keep_tx.as_mut().unwrap().closed().await
    }
}

impl Drop for RFnProvider {
    fn drop(&mut self) {
        // empty
    }
}

/// Calls an async Fn function possibly located on a remote endpoint.
///
/// The remote function can be cloned and executed simultaneously from multiple callers.
/// For each invocation a new async task is spawned.
#[derive(Serialize, Deserialize)]
#[serde(bound(serialize = "A: RemoteSend, R: RemoteSend, Codec: CodecT"))]
#[serde(bound(deserialize = "A: RemoteSend, R: RemoteSend, Codec: CodecT"))]
pub struct RFn<A, R, Codec> {
    request_tx: mpsc::Sender<RFnRequest<A, R, Codec>, Codec, 1>,
}

impl<A, R, Codec> Clone for RFn<A, R, Codec> {
    fn clone(&self) -> Self {
        Self { request_tx: self.request_tx.clone() }
    }
}

impl<A, R, Codec> fmt::Debug for RFn<A, R, Codec> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("RFn").finish_non_exhaustive()
    }
}

impl<A, R, Codec> RFn<A, R, Codec>
where
    A: RemoteSend,
    R: RemoteSend,
    Codec: CodecT,
{
    /// Create a new remote function.
    pub fn new<F, Fut>(fun: F) -> Self
    where
        F: Fn(A) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send,
    {
        let (rfn, provider) = Self::provided(fun);
        provider.keep();
        rfn
    }

    /// Create a new remote function and return it with its provider.
    pub fn provided<F, Fut>(fun: F) -> (Self, RFnProvider)
    where
        F: Fn(A) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send,
    {
        let (request_tx, mut request_rx): (_, mpsc::Receiver<_, _, 1>) = mpsc::channel(1);
        let (keep_tx, keep_rx) = tokio::sync::oneshot::channel();
        let fun = Arc::new(fun);

        tokio::spawn(async move {
            let term = async move {
                if let Ok(()) = keep_rx.await {
                    future::pending().await
                }
            };
            pin_mut!(term);

            loop {
                tokio::select! {
                    biased;

                    () = &mut term => break,

                    req_res = request_rx.recv() => {
                        match req_res {
                            Ok(Some(RFnRequest {argument, result_tx})) => {
                                let fun_task = fun.clone();
                                tokio::spawn(async move {
                                    let result = fun_task(argument).await;
                                    let _ = result_tx.send(result);
                                });
                            }
                            Ok(None) => break,
                            Err(_) => (),
                        }
                    }
                }
            }
        });

        (Self { request_tx }, RFnProvider { keep_tx: Some(keep_tx) })
    }

    /// Try to call the remote function.
    pub async fn try_call(&self, argument: A) -> Result<R, CallError> {
        let (result_tx, result_rx) = oneshot::channel();
        let _ = self.request_tx.send(RFnRequest { argument, result_tx });

        let result = result_rx.await?;
        Ok(result)
    }
}

impl<A, RT, RE, Codec> RFn<A, Result<RT, RE>, Codec>
where
    A: RemoteSend,
    RT: RemoteSend,
    RE: RemoteSend + From<CallError>,
    Codec: CodecT,
{
    /// Call the remote function.
    ///
    /// The [CallError] type must be convertable to the functions error type.
    pub async fn call(&self, argument: A) -> Result<RT, RE> {
        Ok(self.try_call(argument).await??)
    }
}

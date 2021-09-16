//! Lazy transmission of large binary data.

use bytes::Bytes;
use futures::{
    future,
    future::{BoxFuture, MaybeDone},
    FutureExt,
};
use serde::{Deserialize, Serialize};
use std::{convert::TryFrom, fmt, pin::Pin, sync::Arc};
use tokio::sync::Mutex;

use super::mpsc;
use crate::{chmux, chmux::DataBuf, codec::CodecT};

mod fw_bin;

/// The size of the binary data exceeds [usize::MAX] on this platform.
#[derive(Debug, Clone)]
pub struct UsizeExceeded(pub u64);

impl fmt::Display for UsizeExceeded {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "binary data ({} bytes) exceeds maximum array size", self.0)
    }
}

impl std::error::Error for UsizeExceeded {}

/// An error occured fetching the binary data from the remote endpoint.
#[derive(Debug, Clone)]
pub enum FetchError {
    /// The provider has been dropped.
    Dropped,
    /// The size of the binary data exceeds [usize::MAX] on this platform.
    Size(UsizeExceeded),
    /// Receiving the binary data from the remote endpoint failed.
    RemoteReceive(chmux::RecvError),
    /// Connecting a sent channel failed.
    RemoteConnect(super::ConnectError),
}

impl From<UsizeExceeded> for FetchError {
    fn from(err: UsizeExceeded) -> Self {
        Self::Size(err)
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Dropped => write!(f, "provider was dropped"),
            Self::Size(err) => write!(f, "{}", err),
            Self::RemoteReceive(err) => write!(f, "receive error: {}", &err),
            Self::RemoteConnect(err) => write!(f, "connect error: {}", &err),
        }
    }
}

impl std::error::Error for FetchError {}

/// Holds the data for a [LazyBlob].
///
/// Dropping the provider will stop making the data available for remote fetching.
pub struct Provider {
    keep_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Provider").finish_non_exhaustive()
    }
}

impl Provider {
    /// Keeps the provider alive until it is not required anymore.
    pub fn keep(mut self) {
        let _ = self.keep_tx.take().unwrap().send(());
    }

    /// Waits until the provider can be safely dropped.
    ///
    /// This is the case when all associated [LazyBlob]s requested
    /// and received the data or have been dropped.
    pub async fn done(&mut self) {
        self.keep_tx.as_mut().unwrap().closed().await
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        // empty
    }
}

/// Lazily transferred binary data.
#[derive(Clone, Serialize, Deserialize)]
#[serde(bound(serialize = "Codec: CodecT"))]
#[serde(bound(deserialize = "Codec: CodecT"))]
pub struct LazyBlob<Codec> {
    req_tx: mpsc::Sender<fw_bin::Sender, Codec, 1>,
    len: u64,
    #[serde(skip)]
    #[serde(default)]
    #[allow(clippy::type_complexity)]
    fetch_task: Arc<Mutex<Option<Pin<Box<MaybeDone<BoxFuture<'static, Result<DataBuf, FetchError>>>>>>>>,
}

impl<Codec> fmt::Debug for LazyBlob<Codec> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("LazyBlob").field("len", &self.len).finish_non_exhaustive()
    }
}

impl<Codec> LazyBlob<Codec>
where
    Codec: CodecT,
{
    /// Create a new LazyBlob with the specified data.
    ///
    /// The length of the data must not exceed [usize::MAX] on both the sender
    /// and receiver side.
    pub fn new(data: Bytes) -> Self {
        let (lazy_blob, provider) = Self::provided(data);
        provider.keep();
        lazy_blob
    }

    /// Create a new LazyBlob with the specified data and return it together with
    /// its provider.
    pub fn provided(data: Bytes) -> (Self, Provider) {
        let (keep_tx, keep_rx) = tokio::sync::oneshot::channel();
        let (req_tx, mut req_rx): (_, mpsc::Receiver<_, _, 1>) = mpsc::channel(1);
        let len = data.len() as _;

        tokio::spawn(async move {
            let do_send = async move {
                loop {
                    let fw_tx: fw_bin::Sender = match req_rx.recv().await {
                        Ok(Some(fw_tx)) => fw_tx,
                        Ok(None) => break,
                        Err(_) => continue,
                    };

                    let data = data.clone();
                    tokio::spawn(async move {
                        let bin_tx = if let Some(tx) = fw_tx.into_inner() { tx } else { return };
                        let mut tx = if let Ok(tx) = bin_tx.into_inner().await { tx } else { return };
                        let _ = tx.send(data).await;
                    });
                }
            };

            tokio::select! {
                () = do_send => (),
                Err(_) = keep_rx => (),
            }
        });

        let lazy_blob = LazyBlob { req_tx, len, fetch_task: Default::default() };
        let provider = Provider { keep_tx: Some(keep_tx) };
        (lazy_blob, provider)
    }

    /// Returns true if the binary data has zero length.
    pub fn is_empty(&self) -> bool {
        matches!(self.len(), Ok(0))
    }

    /// Returns the length of the binary data.
    ///
    /// This will not fetch the data.
    pub fn len(&self) -> Result<usize, UsizeExceeded> {
        usize::try_from(self.len).map_err(|_| UsizeExceeded(self.len))
    }

    async fn fetch(&self) -> Result<(), FetchError> {
        let mut fetch_task = self.fetch_task.lock().await;

        if fetch_task.is_none() {
            let req_tx = self.req_tx.clone();
            let len = self.len()?;
            *fetch_task = Some(Box::pin(future::maybe_done(
                async move {
                    let (fw_tx, fw_rx) = fw_bin::channel();
                    let _ = req_tx.send(fw_tx).await;
                    let bin_rx = fw_rx.into_inner().await.ok_or(FetchError::Dropped)?;
                    let mut rx = bin_rx.into_inner().await.map_err(FetchError::RemoteConnect)?;
                    rx.set_max_data_size(len);
                    rx.recv().await.map_err(FetchError::RemoteReceive)?.ok_or(FetchError::Dropped)
                }
                .boxed(),
            )));
        }

        fetch_task.as_mut().unwrap().await;

        Ok(())
    }

    /// Returns a shared reference to the binary data.
    ///
    /// The binary data is fetched when this function is first called and
    /// then cached locally.
    pub async fn get(&self) -> Result<DataBuf, FetchError> {
        self.fetch().await?;

        let mut res = self.fetch_task.lock().await;
        res.as_mut().unwrap().as_mut().output_mut().unwrap().clone()
    }

    /// Returns the binary data.
    ///
    /// The binary data is fetched when not already cached by a previous
    /// call to [get](Self::get).
    pub async fn into_inner(mut self) -> Result<DataBuf, FetchError> {
        self.fetch().await?;

        match Arc::try_unwrap(self.fetch_task) {
            Ok(fetch_task) => {
                let mut res = fetch_task.lock().await;
                res.as_mut().unwrap().as_mut().take_output().unwrap()
            }
            Err(shared_fetch_task) => {
                self.fetch_task = shared_fetch_task;
                self.get().await
            }
        }
    }
}

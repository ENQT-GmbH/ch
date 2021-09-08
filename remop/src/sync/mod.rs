use crate::chmux;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{error::Error, fmt};

mod interlock;
pub mod lr;
pub mod mpsc;
pub mod oneshot;
pub mod raw;
mod remote;

/// Error connecting a remote channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectError {
    /// The corresponding sender or receiver has been dropped.
    Dropped,
    /// Error initiating chmux connection.
    Connect(chmux::ConnectError),
    /// Error listening for or accepting chmux connection.
    Listen(chmux::ListenerError),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConnectError::Dropped => write!(f, "other part was dropped"),
            ConnectError::Connect(err) => write!(f, "connect error: {}", err),
            ConnectError::Listen(err) => write!(f, "listen error: {}", err),
        }
    }
}

impl Error for ConnectError {}

/// Object that is remote sendable.
pub trait RemoteSend: Send + Serialize + DeserializeOwned + 'static {}

impl<T> RemoteSend for T where T: Send + Serialize + DeserializeOwned + 'static {}

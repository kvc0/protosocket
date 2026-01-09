use std::sync::Arc;

/// Result type for protosocket-rpc-client.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for protosocket-rpc-client.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    /// Standard IO failure
    #[error("IO failure: {0}")]
    IoFailure(Arc<std::io::Error>),
    /// RPC was cancelled by the remote end
    #[error("Rpc was cancelled remotely")]
    CancelledRemotely,
    /// The connection is closed, no more messages will appear
    #[error("Connection is closed")]
    ConnectionIsClosed,
    /// RPC finished normally
    #[error("Rpc finished")]
    Finished,
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::IoFailure(Arc::new(value))
    }
}

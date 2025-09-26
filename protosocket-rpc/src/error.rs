use std::sync::Arc;

/// Result type for protosocket-rpc-client.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for protosocket-rpc-client.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("IO failure: {0}")]
    IoFailure(Arc<std::io::Error>),
    #[error("Rpc was cancelled remotely")]
    CancelledRemotely,
    #[error("Connection is closed")]
    ConnectionIsClosed,
    #[error("Rpc finished")]
    Finished,
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::IoFailure(Arc::new(value))
    }
}

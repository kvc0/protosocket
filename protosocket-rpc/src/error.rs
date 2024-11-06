/// Result type for protosocket-rpc-client.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for protosocket-rpc-client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO failure: {0}")]
    IoFailure(#[from] std::io::Error),
    #[error("Rpc was cancelled remotely")]
    CancelledRemotely,
    #[error("Connection is closed")]
    ConnectionIsClosed,
    #[error("Rpc finished")]
    Finished,
}

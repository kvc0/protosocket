use std::sync::Arc;

/// Result type for protosocket-server.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for protosocket-server.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    /// Standard IO error
    #[error("IO failure: {0}")]
    IoFailure(#[from] Arc<std::io::Error>),
    /// Address was not able to parse
    #[error("Bad address: {0}")]
    AddressError(#[from] core::net::AddrParseError),
    /// Can not use this resource anymore
    #[error("Requested resource was dead: ({0})")]
    Dead(&'static str),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::IoFailure(Arc::new(e))
    }
}

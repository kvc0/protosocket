use std::sync::Arc;

/// Result type for protosocket-prost.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for protosocket-prost.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("IO failure: {0}")]
    IoFailure(#[from] Arc<std::io::Error>),
    #[error("Bad address: {0}")]
    AddressError(#[from] core::net::AddrParseError),
    #[error("Requested resource was unable to respond: ({0})")]
    Dead(&'static str),
}

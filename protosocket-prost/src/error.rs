use std::sync::Arc;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("IO failure: {0}")]
    IoFailure(#[from] Arc<std::io::Error>),
    #[error("Bad address: {0}")]
    AddressError(#[from] core::net::AddrParseError),
    #[error("Requested resource was unable to respond: ({0})")]
    Dead(&'static str),
}

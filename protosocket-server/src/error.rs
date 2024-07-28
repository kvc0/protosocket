pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO failure: {0}")]
    IoFailure(#[from] std::io::Error),
    #[error("Bad address: {0}")]
    AddressError(#[from] core::net::AddrParseError),
    #[error("Requested resource was dead: ({0})")]
    Dead(&'static str),
}

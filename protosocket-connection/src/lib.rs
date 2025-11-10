//! Low-level connection types for protosocket.
//!
//! This is the core of protosocket, providing the `Connection`
//! type. This type is used to create both client and server
//! channels.
//! Normally you will use `Connection` via the protosocket-prost
//! or protosocket-server crates.

mod connection;
pub mod pooled_encoder;
mod types;

pub use connection::Connection;
pub use pooled_encoder::PooledEncoder;
pub use pooled_encoder::Reusable;
pub use pooled_encoder::Serialize;
pub use types::Decoder;
pub use types::DeserializeError;
pub use types::Encoder;
pub use types::MessageReactor;
pub use types::OwnedBuffer;
pub use types::ReactorStatus;

pub(crate) fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

pub(crate) fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

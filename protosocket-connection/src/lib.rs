//! Low-level connection types for protosocket.
//!
//! This is the core of protosocket, providing the `Connection`
//! type. This type is used to create both client and server
//! channels.
//! Normally you will use `Connection` via the protosocket-prost
//! or protosocket-server crates.

mod connection;
mod types;

pub use connection::Connection;
pub use types::ConnectionBindings;
pub use types::DeserializeError;
pub use types::Deserializer;
pub use types::MessageReactor;
pub use types::ReactorStatus;
pub use types::Serializer;

pub(crate) fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

pub(crate) fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

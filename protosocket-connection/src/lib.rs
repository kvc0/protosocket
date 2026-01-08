//! Low-level connection types for protosocket.
//!
//! This is the core of protosocket, providing the `Connection`
//! type. This type is used to create both client and server
//! channels.
//! Normally you will use `Connection` via the protosocket-prost
//! or protosocket-server crates.

#![deny(missing_docs)]

mod connection;
mod encoding;
mod error;
mod message_reactor;
mod pooled_encoder;
mod socket_listener;

pub use connection::Connection;
pub use encoding::Codec;
pub use encoding::Decoder;
pub use encoding::Encoder;
pub use encoding::OwnedBuffer;
pub use error::DeserializeError;
pub use message_reactor::MessageReactor;
pub use message_reactor::ReactorStatus;
pub use pooled_encoder::PooledEncoder;
pub use pooled_encoder::Reusable;
pub use pooled_encoder::Serialize;
pub use socket_listener::SocketListener;
pub use socket_listener::SocketResult;
pub use socket_listener::StreamWithAddress;
pub use socket_listener::TcpSocketListener;
pub use socket_listener::TlsSocketListener;

pub(crate) fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

pub(crate) fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

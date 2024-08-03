mod connection;
mod connection_driver;
mod types;

pub use connection::Connection;
pub use connection::NetworkStatusEvent;
pub use connection_driver::ConnectionDriver;
pub use connection_driver::MessageReactor;
pub use connection_driver::ReactorStatus;
pub use types::ConnectionBindings;
pub use types::DeserializeError;
pub use types::Deserializer;
pub use types::Serializer;

pub(crate) fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

pub(crate) fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

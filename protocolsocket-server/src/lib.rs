pub(crate) mod connection_server;
pub(crate) mod error;
pub(crate) mod listener_server;

pub use connection_server::ConnectionLifecycle;
pub use connection_server::ConnectionServer;
pub use connection_server::DeserializeError;
pub use connection_server::Deserializer;
pub use connection_server::Serializer;
pub use error::Error;
pub use error::Result;
pub use listener_server::Server;

pub(crate) fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

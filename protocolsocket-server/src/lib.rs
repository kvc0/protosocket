pub(crate) mod connection_server;
pub(crate) mod error;
pub(crate) mod listener_server;

pub use listener_server::Server;
pub use connection_server::ConnectionLifecycle;
pub use connection_server::ConnectionServer;
pub use error::Error;
pub use error::Result;

pub(crate) fn interrupted(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::Interrupted
}

//! Conveniences for writing protosocket servers.
//!
//! See example-telnet for the simplest full example of the entire workings,
//! or example-proto for an example of how to use this crate with protocol buffers.

pub(crate) mod connection_server;
pub(crate) mod error;

pub use connection_server::ProtosocketServer;
pub use connection_server::ServerConnector;
pub use error::Error;
pub use error::Result;

//! Conveniences for using protocol buffers via `prost` with `protosocket`.
//!
//! See the example-proto directory for a complete example of how to use this crate.

mod error;
mod prost_client_registry;
mod prost_serializer;
mod prost_socket;

pub use error::{Error, Result};
pub use prost_client_registry::ClientRegistry;
pub use prost_serializer::ProstSerializer;
pub use prost_socket::ProstClientConnectionBindings;
pub use prost_socket::ProstServerConnectionBindings;

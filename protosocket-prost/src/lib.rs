//! Conveniences for using protocol buffers via `prost` with `protosocket`.
//!
//! See the example-proto directory for a complete example of how to use this crate.

#![deny(missing_docs)]

mod error;
mod prost_client_registry;
mod prost_serializer;

pub use error::{Error, Result};
pub use prost_client_registry::ClientRegistry;
pub use prost_serializer::{ProstDecoder, ProstSerializer};

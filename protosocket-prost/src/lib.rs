//! Conveniences for using protocol buffers via `prost` with `protosocket`.
//!
//! See the example-proto directory for a complete example of how to use this crate.

#![deny(missing_docs)]

mod decoder;
mod error;
mod serializer;

pub use decoder::ProstDecoder;
pub use error::{Error, Result};
pub use serializer::ProstSerializer;

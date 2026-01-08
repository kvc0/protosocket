//! This crate provides messagepack bindings for use with protosocket.

#![deny(missing_docs)]

mod decoder;
mod serializer;

pub use decoder::ProtosocketMessagePackDecoder;
pub use serializer::MessagePackSerializer;

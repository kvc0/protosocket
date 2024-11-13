//! Protosocket RPC client
//!
//! This crate provides an rpc-style client for the protosocket protocol.
//! You can use whatever encoding you want, but you must provide both a
//! `Serializer` and a `Deserializer` for your messages. If you use `prost`,
//! you can use the `protosocket-prost` crate to provide these implementations.
//!
//! Messages must provide a `Message` implementation, which includes a `message_id`
//! and a `control_code`. The `message_id` is used to correlate requests and responses,
//! while the `control_code` is used to provide special handling for messages. Currently
//! the only special handling is cancellation.
//!
//! This RPC client is medium-low level wrapper around the low level protosocket crate,
//! adding a layer of RPC semantics. You are expected to write a wrapper with the functions
//! that make sense for your application, and use this client as the transport layer.

mod error;
mod message;
mod reactor;

pub mod client;
pub mod server;

pub use error::{Error, Result};
pub use message::{Message, ProtosocketControlCode};
pub use reactor::completion_streaming::StreamingCompletion;
pub use reactor::completion_unary::UnaryCompletion;

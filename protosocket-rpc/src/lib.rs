//! Protosocket RPC
//!
//! This crate provides an rpc-style client and server for the protosocket
//! protocol. You can use whatever encoding you want, but you must provide both a
//! `Serializer` and a `Deserializer` for your messages. If you use `prost`,
//! you can use the `protosocket-prost` crate to provide these implementations.
//! See example-proto for an example of how to use this crate with protocol buffers.
//!
//! Messages must provide a `Message` implementation, which includes a `message_id`
//! and a `control_code`. The `message_id` is used to correlate requests and responses,
//! while the `control_code` is used to provide special handling for messages. Currently
//! the only special handling is cancellation.
//!
//! This RPC client is medium-low level wrapper around the low level protosocket crate,
//! adding a layer of RPC semantics. You are expected to write a wrapper with the functions
//! that make sense for your application, and use this client as the transport layer.
//!
//! Clients and servers need to agree about the request and response semantics. While it is
//! supported to have dynamic streaming/unary response types, it is recommended to instead
//! use separate rpc-initiating messages for streaming and unary rpcs.

mod error;
mod message;

pub mod client;
pub mod server;

pub use error::{Error, Result};
pub use message::{Message, ProtosocketControlCode};

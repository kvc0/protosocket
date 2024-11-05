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

mod completion_reactor;
mod completion_streaming;
mod completion_unary;
mod configuration;
mod error;
mod message;
mod rpc_client;
mod rpc_drop_guard;

pub use completion_streaming::StreamingCompletion;
pub use completion_unary::UnaryCompletion;
pub use configuration::{connect, Configuration};
pub use error::{Error, Result};
pub use message::{Message, ProtosocketControlCode};
pub use rpc_client::RpcClient;

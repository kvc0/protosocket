//! Protosocket RPC
//!
//! This crate provides an rpc-style client and server for protosocket
//! connections. You can use whatever encoding you want, but you must provide both
//! an `Encoder` and a `Decoder` for your messages. If you use `prost`, you can use
//! the `protosocket-prost` crate to provide these implementations. There's also
//! a `protosocket-messagepack` available, and other encodings are similarly
//! straightforward to add.
//!
//! * See example-proto for an example of how to use this crate with protocol buffers.
//! * See example-messagepack for an example of how to use this crate with messagepack.
//!
//! Messages must provide a `Message` implementation, which includes a `message_id`
//! and a `control_code`. The `message_id` is used to correlate requests and responses,
//! while the `control_code` is used to provide special handling for messages. You can
//! receive Cancel when an rpc is aborted, and End when a streaming rpc is complete.
//!
//! This RPC client is medium-low level wrapper around the low level protosocket crate,
//! adding a layer of RPC semantics. You are expected to write a wrapper with the functions
//! that make sense for your application, and use this client as the transport layer.
//!
//! RPC cancellation is supported.
//!
//! Clients and servers need to agree about the request and response semantics. While it is
//! supported to have dynamic streaming/unary response types, it is recommended to instead
//! use separate rpc-initiating messages for streaming and unary rpcs.

#![deny(missing_docs)]

mod error;
mod message;

pub mod client;
pub mod server;

pub use error::{Error, Result};
pub use message::{Message, ProtosocketControlCode};

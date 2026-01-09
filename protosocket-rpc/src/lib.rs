#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

mod error;
mod message;

pub mod client;
pub mod server;

pub use error::{Error, Result};
pub use message::{Message, ProtosocketControlCode};

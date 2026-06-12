//! The libfabric (EFA / RDMA) transport implementation.
//!
//! Layering: [`provider`] picks the libfabric provider; [`fabric`] (over the
//! raw [`ffi`]) owns the libfabric object graph; [`bootstrap`] exchanges fabric
//! addresses over TCP; [`driver`] reaps completions and carries bytes; and
//! [`listener`]/[`stream`] expose the [`EfaSocketListener`]/[`EfaStream`] types.

mod bootstrap;
mod driver;
mod error;
mod fabric;
mod ffi;
mod listener;
mod provider;
mod stream;

pub use error::EfaError;
pub use listener::EfaSocketListener;
pub use provider::Provider;
pub use stream::EfaStream;

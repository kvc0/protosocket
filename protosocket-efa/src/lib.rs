//! A [`protosocket::SocketListener`] implemented over [libfabric], enabling
//! protosocket servers to run on RDMA fabrics — most notably AWS EFA
//! (Elastic Fabric Adapter), but also any other libfabric provider such as
//! `tcp`, `udp`, `verbs`, or `shm`.
//!
//! # Why this is not "just a socket"
//!
//! libfabric's reliable endpoints expose *message* (datagram) semantics with
//! **no native `connect`/`accept`**. This crate builds a connection-oriented,
//! ordered byte stream on top of reliable-datagram messaging:
//!
//! 1. A [`Provider`] selects the libfabric provider to use.
//! 2. Connections are bootstrapped over a TCP side-channel that exchanges raw
//!    fabric addresses. Once peers know each other's address, data flows over
//!    the fabric.
//! 3. Each established connection is surfaced as an [`EfaStream`], which
//!    implements [`tokio::io::AsyncRead`] + [`tokio::io::AsyncWrite`] so it
//!    plugs directly into protosocket's `Connection`.
//!
//! The server side is [`EfaSocketListener`], which implements
//! [`protosocket::SocketListener`]; the client side is [`EfaStream::connect`].
//!
//! The libfabric implementation lives in the [`efa`] module.
//!
//! [libfabric]: https://ofiwg.github.io/libfabric/

#![deny(missing_docs)]

mod efa;

pub use efa::EfaError;
pub use efa::EfaSocketListener;
pub use efa::EfaStream;
pub use efa::Provider;

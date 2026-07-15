//! Types and tools for `protosocket-rpc` servers.
//!
//! See example-proto or example-messagepack for how to make servers.

mod abortion_tracker;
mod rpc_stream;
mod rpc_submitter;
mod server_traits;
mod socket_server;
mod spawn;

pub use server_traits::{ConnectionService, RpcKind, SocketService};
pub use socket_server::SocketRpcServer;
pub use spawn::{LevelSpawn, Spawn, TokioSpawn};

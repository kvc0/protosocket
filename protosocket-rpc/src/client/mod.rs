//! Types and tools for `protosocket-rpc` clients.
//! 
//! See example-proto or example-messagepack for how to make clients.

mod configuration;
mod connection_pool;
mod reactor;
mod rpc_client;
mod stream_connector;

pub use configuration::{
    connect, Configuration
};
pub use rpc_client::RpcClient;
pub use stream_connector::{
    StreamConnector, TcpStreamConnector, UnverifiedTlsStreamConnector,
    WebpkiTlsStreamConnector,
};

pub use reactor::completion_streaming::StreamingCompletion;
pub use reactor::completion_unary::UnaryCompletion;

pub use connection_pool::ClientConnector;
pub use connection_pool::ConnectionPool;

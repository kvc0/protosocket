use std::{future::Future, net::SocketAddr};

use protosocket::{Deserializer, Serializer};

use crate::Message;

pub enum RpcKind<Unary, Streaming> {
    Unary(Unary),
    Streaming(Streaming),
}

pub trait RpcResponse {
    type Response: Message;

    fn into_kind(
        self,
    ) -> RpcKind<
        impl Future<Output = crate::Result<Self::Response>>,
        impl futures::Stream<Item = crate::Result<Self::Response>>,
    >;
}

/// A server connection receives rpcs from clients and sends responses
pub trait ConnectionServer: Send + Unpin + 'static {
    type Request: Message;
    type Response: Message;
    type UnaryCompletion: Future<Output = crate::Result<Self::Response>> + Send + Unpin;
    type StreamingCompletion: futures::Stream<Item = crate::Result<Self::Response>> + Send + Unpin;

    /// Spawn a new rpc and arrange for it to complete.
    ///
    /// You can provide a concrete Future and it will be polled in the context of the Connection
    /// itself. This would limit your Connection and all of its outstanding rpc's to 1 cpu at a time.
    /// That might be good for your use case, or it might be bad.
    /// You can of course also spawn a task and return a completion future that completes when the
    /// task completes, e.g., with a tokio::sync::oneshot or mpsc stream.
    fn new_rpc(
        &self,
        initiating_message: Self::Request,
    ) -> RpcKind<Self::UnaryCompletion, Self::StreamingCompletion>;
}

/// Notified when a new connection is established, to get an RPC spawner for the connection.
/// This can be used to handle the lifecycle of a connection.
pub trait SocketServer: 'static {
    type RequestDeserializer: Deserializer<Message: Message> + 'static;
    type ResponseSerializer: Serializer<Message: Message> + 'static;
    type ConnectionServer: ConnectionServer<
        Request = <Self::RequestDeserializer as Deserializer>::Message,
        Response = <Self::ResponseSerializer as Serializer>::Message,
    >;

    fn deserializer(&self) -> Self::RequestDeserializer;
    fn serializer(&self) -> Self::ResponseSerializer;

    fn new_connection_server(&self, address: SocketAddr) -> Self::ConnectionServer;
}

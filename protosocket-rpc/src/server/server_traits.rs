use std::{future::Future, marker::PhantomData};

use protosocket::{Connection, Decoder, Encoder, SocketListener};

use crate::{
    server::{connection_server::RpcConnectionServer, rpc_submitter::RpcSubmitter},
    Message,
};

/// SocketService receives connections and produces ConnectionServices.
///
/// The SocketService is notified when a new connection is established. It is given the address of the
/// remote peer and it returns a ConnectionService for that connection. You can think of this as the
/// "connection factory" for your server. It is the "top" of your service stack.
pub trait SocketService: 'static {
    /// The type of deserializer for incoming messages.
    type RequestDecoder: Decoder<Message: Message> + 'static;
    /// The type of serializer for outgoing messages.
    ///
    /// Consider pooling your allocations, like with `protosocket::PooledEncoder`.
    /// The write out to the network uses the raw `Encoder::Serialized` type, so you
    /// can make outbound messages low-allocation via simple pooling.
    type ResponseEncoder: Encoder<Message: Message> + 'static;
    /// The type of connection service that will be created for each connection.
    type ConnectionService: ConnectionService<
        Request = <Self::RequestDecoder as Decoder>::Message,
        Response = <Self::ResponseEncoder as Encoder>::Message,
    >;

    /// The listener type for this service. E.g., `TcpSocketListener`
    type SocketListener: SocketListener;

    /// Create a new deserializer for incoming messages.
    fn decoder(&self) -> Self::RequestDecoder;
    /// Create a new serializer for outgoing messages.
    fn encoder(&self) -> Self::ResponseEncoder;

    /// Create a new ConnectionService for your new connection.
    /// The Stream will be wired into a `protosocket::Connection`. You can look at it in here
    /// if it has data you want (like a SocketAddr).
    fn new_stream_service(
        &self,
        _stream: &<Self::SocketListener as SocketListener>::Stream,
    ) -> Self::ConnectionService;
}

/// A strategy for spawning connections.
/// Some Connections may not be Send
pub trait SpawnConnection<TSocketService: SocketService>: Clone {
    /// Spawn a connection driver task and a connection server task.
    fn spawn_connection(
        &self,
        connection: Connection<
            <<TSocketService as SocketService>::SocketListener as SocketListener>::Stream,
            TSocketService::RequestDecoder,
            TSocketService::ResponseEncoder,
            RpcSubmitter<TSocketService>,
        >,
        server: RpcConnectionServer<<TSocketService as SocketService>::ConnectionService>,
    );
}

/// When everything in your `SocketService` is `Send`, you can use a TokioSpawnConnection.
#[derive(Debug)]
pub struct TokioSpawnConnection<TSocketService> {
    _phantom: PhantomData<TSocketService>,
}
impl<TSocketService> Default for TokioSpawnConnection<TSocketService> {
    fn default() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}
impl<TSocketService> Clone for TokioSpawnConnection<TSocketService> {
    fn clone(&self) -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}
impl<TSocketService> SpawnConnection<TSocketService> for TokioSpawnConnection<TSocketService>
where
    TSocketService: SocketService,
    TSocketService::RequestDecoder: Send,
    TSocketService::ResponseEncoder: Send,
    <TSocketService::ResponseEncoder as Encoder>::Serialized: Send,
    <TSocketService::SocketListener as SocketListener>::Stream: Send,
    <TSocketService::ConnectionService as ConnectionService>::UnaryFutureType: Send,
    <TSocketService::ConnectionService as ConnectionService>::StreamType: Send,
{
    fn spawn_connection(
        &self,
        connection: Connection<
            <<TSocketService as SocketService>::SocketListener as SocketListener>::Stream,
            <TSocketService as SocketService>::RequestDecoder,
            <TSocketService as SocketService>::ResponseEncoder,
            RpcSubmitter<TSocketService>,
        >,
        server: RpcConnectionServer<<TSocketService as SocketService>::ConnectionService>,
    ) {
        tokio::spawn(connection);
        tokio::spawn(server);
    }
}

/// A connection service receives rpcs from clients and sends responses.
///
/// Each client connection gets a ConnectionService. You put your per-connection state in your
/// ConnectionService implementation.
///
/// Every interaction with a client is done via an RPC. You are called with the initiating message
/// from the client, and you return the kind of response future that is used to complete the RPC.
///
/// A ConnectionService is executed in the context of an RPC connection server, which is a future.
/// This means you get `&mut self` when you are called with a new rpc. You can use simple mutable
/// state per-connection; but if you need to share state between connections or elsewhere in your
/// application, you will need to use an appropriate state sharing mechanism.
pub trait ConnectionService: Send + Unpin + 'static {
    /// The type of request message, These messages initiate rpcs.
    type Request: Message;
    /// The type of response message, These messages complete rpcs, or are streamed from them.
    type Response: Message;
    /// The type of future that completes a unary rpc.
    type UnaryFutureType: Future<Output = Self::Response> + Unpin;
    /// The type of stream that completes a streaming rpc.
    type StreamType: futures::Stream<Item = Self::Response> + Unpin;

    /// Create a new rpc task completion.
    ///
    /// You can provide a concrete Future and it will be polled in the context of the Connection
    /// itself. This would limit your Connection and all of its outstanding rpc's to 1 cpu at a time.
    /// That might be good for your use case, or it might be suboptimal.
    /// You can of course also spawn a task and return a completion future that completes when the
    /// task completes, e.g., with a tokio::sync::oneshot or mpsc stream. In general, try to do as
    /// little as possible: Return a future (rather than a task handle) and let the ConnectionServer
    /// task poll it. This keeps your task count low and your wakes more tightly related to the
    /// cooperating tasks (e.g., ConnectionServer and Connection) that need to be woken.
    fn new_rpc(
        &mut self,
        initiating_message: Self::Request,
    ) -> RpcKind<Self::UnaryFutureType, Self::StreamType>;
}

/// Type of rpc to be awaited
pub enum RpcKind<Unary, Streaming> {
    /// This is a unary rpc. It will complete with a single response.
    Unary(Unary),
    /// This is a streaming rpc. It will complete with a stream of responses.
    Streaming(Streaming),
    /// This is an unknown rpc. It will be skipped.
    Unknown,
}

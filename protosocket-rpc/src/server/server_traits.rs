use std::{future::Future, pin::pin};

use protosocket::{Decoder, Encoder, SocketListener};

use crate::Message;

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
pub trait ConnectionService: Unpin + 'static {
    /// The type of request message, These messages initiate rpcs.
    type Request: Message;
    /// The type of response message, These messages complete rpcs, or are streamed from them.
    type Response: Message;

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
    ) -> RpcKind<impl Future<Output = Self::Response>, impl futures::Stream<Item = Self::Response>>;
}

#[must_use]
pub struct RpcResponder<Response> {
    outbound: spillway::Sender<Response>,
}
impl<Response> RpcResponder<Response> {
    pub fn unary(self, unary_rpc: impl Future<Output = Response>) -> impl Future<Output = ()> {
        async move {
            let response = unary_rpc.await;
            if self.outbound.send(response).is_err() {
                log::debug!("outbound channel is closed");
            }
        }
    }

    pub fn streaming(self, streaming_rpc: impl futures::Stream<Item = Response>) -> impl Future<Output = ()> {
        async move {
            let mut streaming_rpc = pin!(streaming_rpc);
            while let Some(next) = futures::StreamExt::next(&mut streaming_rpc).await {
                if self.outbound.send(next).is_err() {
                    log::debug!("outbound channel closed during streaming response");
                    break;
                }
            }
        }
    }

    pub fn immediate(self, response: Response) {
        if self.outbound.send(response).is_err() {
            log::debug!("outbound channel closed while sending response");
        }
    }
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

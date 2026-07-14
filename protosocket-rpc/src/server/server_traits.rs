use std::future::Future;

use futures::Stream;
use protosocket::{Codec, Decoder, Encoder, SocketListener};

use crate::Message;

/// SocketService receives connections and produces ConnectionServices.
///
/// The SocketService is notified when a new connection is established. It is given the address of the
/// remote peer and it returns a ConnectionService for that connection. You can think of this as the
/// "connection factory" for your server. It is the "top" of your service stack.
pub trait SocketService: 'static {
    /// Message encoding scheme
    ///
    /// Consider pooling your allocations, like with `protosocket::PooledEncoder`.
    /// The write out to the network uses the raw `Encoder::Serialized` type, so you
    /// can make outbound messages low-allocation via simple pooling.
    type Codec: Codec + Decoder<Message: Message> + Encoder<Message: Message>;

    /// The type of connection service that will be created for each connection.
    type ConnectionService: ConnectionService<
        Request = <Self::Codec as Decoder>::Message,
        Response = <Self::Codec as Encoder>::Message,
    >;

    /// The listener type for this service. E.g., `TcpSocketListener`
    type SocketListener: SocketListener;

    /// Create a new message codec for a connection.
    fn codec(&self) -> Self::Codec;

    /// Create a new ConnectionService for your new connection.
    /// The Stream will be wired into a `protosocket::Connection`. You can look at it in here
    /// if it has data you want (like a SocketAddr).
    fn new_stream_service(
        &self,
        _stream: &<Self::SocketListener as SocketListener>::Stream,
    ) -> Self::ConnectionService;
}

/// A connection service receives rpcs from clients and returns the work that produces responses.
///
/// Each client connection gets a ConnectionService. You put your per-connection state in your
/// ConnectionService implementation.
///
/// Every interaction with a client is done via an RPC. You are called with the initiating message
/// from the client, and you return the kind of rpc that completes it: a future for a unary rpc,
/// or a stream for a streaming rpc.
///
/// Your futures and streams are driven by the connection itself, and they are only polled when
/// the connection has room to send responses. This is how backpressure works: when the peer
/// stops receiving, your rpcs stop being polled. If your stream models lag or load-shedding
/// (like `tokio::sync::broadcast`), a slow peer causes that model to engage instead of buffering
/// responses without bound.
///
/// Because rpcs are polled in the context of the connection, a connection and all of its
/// outstanding rpcs advance on 1 cpu at a time. That might be good for your use case, or it
/// might be suboptimal. If an rpc is compute-heavy, you can of course spawn a task and return
/// a future that completes when the task completes, e.g., with a `tokio::sync::oneshot`. In
/// general, try to do as little as possible: Return a future and let the connection poll it.
/// This keeps your task count low and your wakes more tightly related to the cooperating
/// tasks that need to be woken.
///
/// Response message ids are stamped by the connection: every response or stream item is sent
/// with the message id of the rpc that initiated it.
pub trait ConnectionService: Unpin + 'static {
    /// The type of request message, These messages initiate rpcs.
    type Request: Message;
    /// The type of response message, These messages complete rpcs, or are streamed from them.
    type Response: Message;
    /// The type of future that completes a unary rpc. Use a `BoxFuture` if your future
    /// is not `Unpin`.
    type UnaryFutureType: Future<Output = Self::Response> + Unpin;
    /// The type of stream that produces the responses for a streaming rpc. Use a
    /// `BoxStream` if your stream is not `Unpin`.
    type StreamType: Stream<Item = Self::Response> + Unpin;

    /// Called with an initiating message from the client. Return the rpc completion for the
    /// message: `RpcKind::Unary` for a single response, `RpcKind::Streaming` for a stream of
    /// responses, or `RpcKind::Cancelled` to reject the message (the client receives a
    /// cancellation for it).
    fn new_rpc(
        &mut self,
        initiating_message: Self::Request,
    ) -> RpcKind<Self::UnaryFutureType, Self::StreamType>;

    /// Optional poll to allow the connection to push work forward internally.
    ///
    /// This is called unconditionally on every connection poll - it is NOT subject to
    /// send-capacity backpressure. Do not produce outbound messages from here; use it for
    /// bookkeeping, timers, and other connection state machines.
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::ops::ControlFlow<()> {
        std::ops::ControlFlow::Continue(())
    }
}

/// Type of rpc to be completed
pub enum RpcKind<Unary, Streaming> {
    /// This is a unary rpc. It will complete with a single response.
    Unary(Unary),
    /// This is a streaming rpc. It will complete with a stream of responses.
    Streaming(Streaming),
    /// Do not process this rpc. The initiating message is answered with a cancellation.
    Cancelled,
}

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
    /// The type of connection service that will be created for each connection.
    type ConnectionService: ConnectionService;

    /// The listener type for this service. E.g., `TcpSocketListener`
    type SocketListener: SocketListener;

    /// Create a new message codec for a connection.
    fn codec(&mut self) -> <Self::ConnectionService as ConnectionService>::Codec;

    /// Create a new ConnectionService for your new connection.
    /// The Stream will be wired into a `protosocket::Connection`. You can look at it in here
    /// if it has data you want (like a SocketAddr).
    fn new_stream_service(
        &mut self,
        _stream: &<Self::SocketListener as SocketListener>::Stream,
    ) -> Self::ConnectionService;
}

/// A connection service receives rpcs from clients and returns the work that produces
/// responses.
///
/// Each client connection gets a ConnectionService; put your per-connection state here.
/// You are called with each initiating message, and you return the rpc that completes it:
/// a future for a unary rpc, or a stream for a streaming rpc.
///
/// The connection drives your rpcs, and only polls them when it has room to send. A peer
/// that stops receiving stops its rpcs. If your stream models lag (like
/// `tokio::sync::broadcast`), a slow peer engages that model instead of buffering
/// responses without bound.
///
/// Rpcs are polled in the connection's task. If an rpc is compute-heavy, you can spawn
/// it and return a completion future or a channel-backed stream.
///
/// Response message ids are stamped by the connection.
pub trait ConnectionService: Unpin + 'static {
    /// The wire codec for connections hosting this service.
    ///
    /// Consider pooling your allocations, like with `protosocket::PooledEncoder`.
    /// The write out to the network uses the raw `Encoder::Serialized` type, so you
    /// can make outbound messages low-allocation via simple pooling.
    type Codec: Codec
        + Decoder<Message = Self::Request>
        + Encoder<Message = Self::Response>
        + 'static;
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
    /// A rpc that produces a single response.
    Unary(Unary),
    /// A rpc that produces a stream of responses.
    Streaming(Streaming),
    /// An immediate cancellation of an rpc.
    Cancelled,
}

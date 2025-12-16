use protosocket::{Codec, Decoder, Encoder, SocketListener};

use crate::{server::rpc_responder::RpcResponder, Message};

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
    /// You send the response via the RpcResponder.
    fn new_rpc(
        &mut self,
        initiating_message: Self::Request,
        responder: RpcResponder<'_, Self::Response>,
    );

    /// Optional poll to allow the connection to push work forward internally.
    ///
    /// You can use this to drive connection state machines (e.g., `FuturesUnordered`),
    /// or whatever else you need to do with your connection between reading from the network and
    /// writing to it.
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::ops::ControlFlow<()> {
        std::ops::ControlFlow::Continue(())
    }
}

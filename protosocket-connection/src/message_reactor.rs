/// A message reactor is a stateful object that processes inbound messages.
/// You receive &mut self, and you receive your messages by value.
///
/// A message reactor may be a server which spawns a task per message, or a client which
/// matches response ids to a HashMap of concurrent requests with oneshot completions.
///
/// Your message reactor and your tcp connection share their fate - when one drops or
/// disconnects, the other does too.
pub trait MessageReactor: 'static {
    /// Messages inbound from the remote.
    type Inbound;
    /// Messages outbound to a remote.
    type Outbound;
    /// Messages in-memory, delivered to the reactor before serialization.
    type LogicalOutbound;

    /// Called from the connection's driver task when messages are received.
    ///
    /// You must take the message quickly: Blocking here will block the connection.
    /// If you can't accept new messages and you can't queue, you should consider returning
    /// Disconnect.
    fn on_inbound_message(&mut self, message: Self::Inbound) -> ReactorStatus;

    /// Optional poll to allow the reactor to push work forward internally.
    ///
    /// You can use this to drive connection state machines (e.g., `FuturesUnordered`),
    /// or whatever else you need to do with your reactor between reading from the network and
    /// writing to it.
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::ops::ControlFlow<()> {
        std::ops::ControlFlow::Continue(())
    }

    /// Called from the connection's driver task when messages are sent.
    ///
    /// You can use this to track outbound messages, or for logging, or metrics, or whatever.
    fn on_outbound_message(&mut self, message: Self::LogicalOutbound) -> Self::Outbound;

    /// Poll for the next outbound message produced by the reactor itself.
    ///
    /// This is only called when the connection has room in its send queue. This is how
    /// backpressure is applied to reactor-driven work: when the connection cannot write,
    /// the reactor is not polled for outbound messages, and any streams or futures the
    /// reactor drives to produce them are not advanced. If your source of messages models
    /// lag or load-shedding (like `tokio::sync::broadcast`), a slow or stalled peer causes
    /// that model to engage instead of buffering without bound.
    ///
    /// Return `Poll::Ready(None)` to tell the connection the reactor is finished producing
    /// messages and the connection should close. If your reactor does not produce messages
    /// on its own (for example, a client whose outbound messages are all submitted through
    /// the connection's outbound queue), leave the default implementation.
    fn poll_next_outbound(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::LogicalOutbound>> {
        std::task::Poll::Pending
    }
}

/// What the connection should do after processing a batch of inbound messages.
#[derive(Debug, PartialEq, Eq)]
pub enum ReactorStatus {
    /// Continue processing messages.
    Continue,
    /// Disconnect the tcp connection.
    Disconnect,
}

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

    /// Called from the connection's driver task when messages from the outbound queue
    /// are sent. Messages produced by `poll_outbound_many` do not come through here.
    ///
    /// You can use this to track outbound messages, or for logging, or metrics, or whatever.
    fn on_outbound_message(&mut self, message: Self::LogicalOutbound) -> Self::Outbound;

    /// Produce outbound wire messages into `outbound`.
    ///
    /// This is only called when the connection has room to send, and `outbound` refuses
    /// messages beyond that room. When the connection cannot write, work that produces
    /// messages is not advanced. If your message source models lag (like
    /// `tokio::sync::broadcast`), a slow peer engages that model instead of buffering
    /// without bound.
    ///
    /// Messages you emit here are yours: do any bookkeeping before you send them.
    ///
    /// Return `Ready(Some(()))` after emitting, `Pending` when nothing is available
    /// right now, and `Ready(None)` if you will never emit - the default. The connection
    /// closes when its outbound queue is closed and this returns `Ready(None)`.
    fn poll_outbound_many(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        _outbound: &mut impl SendBudget<Self::Outbound>,
    ) -> std::task::Poll<Option<()>> {
        std::task::Poll::Ready(None)
    }
}

/// A bounded lease on a connection's send capacity.
pub trait SendBudget<T> {
    /// Take a permit to send one message. None when the budget is spent.
    /// Dropping a permit unused returns its capacity.
    fn reserve(&mut self) -> Option<impl SendPermit<T> + '_>;

    /// Reserve up to `count` sends. The permit reports how many it actually holds -
    /// possibly zero, possibly fewer than requested. Unspent sends return to the
    /// budget when the permit drops.
    fn reserve_many(&mut self, count: usize) -> impl SendManyPermit<T> + '_;
}

/// A reserved slot in a connection's send queue.
pub trait SendPermit<T> {
    /// Spend the permit.
    fn send(self, message: T);
}

/// A reserved run of slots in a connection's send queue.
pub trait SendManyPermit<T> {
    /// How many sends this permit holds.
    fn reserved(&self) -> usize;

    /// Spend one of the reserved sends.
    ///
    /// Sending more than `reserved` messages panics, like indexing out of bounds:
    /// check `reserved` and send at most that many.
    fn send(&mut self, message: T);
}

/// What the connection should do after processing a batch of inbound messages.
#[derive(Debug, PartialEq, Eq)]
pub enum ReactorStatus {
    /// Continue processing messages.
    Continue,
    /// Disconnect the tcp connection.
    Disconnect,
}

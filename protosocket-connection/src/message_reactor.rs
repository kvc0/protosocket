use crate::{encoding::Codec, send_budget::SendBudget, Decoder, Encoder};

/// A message reactor is a stateful object that processes inbound messages.
/// You receive &mut self, and you receive your messages by value.
///
/// A message reactor may be a server which spawns a task per message, or a client which
/// matches response ids to a HashMap of concurrent requests with oneshot completions.
///
/// Your message reactor and your tcp connection share their fate - when one drops or
/// disconnects, the other does too.
pub trait MessageReactor: 'static {
    /// The wire codec for this reactor's connection.
    type Codec: Codec + Decoder<Message = Self::Inbound> + Encoder<Message = Self::Outbound>;
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
    ///
    /// This is polled unconditionaly. For producing outbound wire messages, you should implement
    /// [poll_outbound_many], which exerts backpressure when outbound writes are blocked.
    fn poll(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
    ) -> std::ops::ControlFlow<()> {
        std::ops::ControlFlow::Continue(())
    }

    /// Called from the connection's driver task when messages from the outbound queue
    /// are sent.
    ///
    /// You can use this to track outbound messages, or for logging, or metrics, or whatever.
    ///
    /// Not invoked for messages produced by `poll_outbound_many`. Do any required bookkeeping
    /// within that method.
    fn on_outbound_message(&mut self, message: Self::LogicalOutbound) -> Self::Outbound;

    /// Produce outbound wire messages.
    ///
    /// This is only called when the connection has room to send, and `outbound` refuses
    /// messages beyond that room. When the connection cannot write, work that produces
    /// messages is not advanced.
    ///
    /// Messages emitted here do not trigger `on_outbound_message`: do any required bookkeeping
    /// within this method.
    ///
    /// Return `Ready(Some(()))` after emitting, `Pending` when nothing is available
    /// right now, and `Ready(None)` if you will never emit. The connection
    /// closes when both its outbound queue is closed and this method returns `Ready(None)`.
    fn poll_outbound_many(
        self: std::pin::Pin<&mut Self>,
        _context: &mut std::task::Context<'_>,
        _outbound: &mut SendBudget<'_, Self::Codec>,
    ) -> std::task::Poll<Option<()>> {
        std::task::Poll::Ready(None)
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

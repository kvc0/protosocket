use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// A message reactor is a stateful object that processes inbound messages.
/// You receive &mut self, and you receive your messages by value.
///
/// A message reactor may be a server which spawns a task per message, or a client which
/// matches response ids to a HashMap of concurrent requests with oneshot completions.
///
/// Your message reactor and your tcp connection share their fate - when one drops or
/// disconnects, the other does too.
pub trait MessageReactor: 'static {
    type Inbound;

    /// Called from the connection's driver task when messages are received.
    ///
    /// You must take the message quickly: Blocking here will block the connection.
    /// If you can't accept new messages and you can't queue, you should consider returning
    /// Disconnect.
    fn on_inbound_message(&mut self, message: Self::Inbound) -> ReactorStatus;

    /// Called by the driver task
    ///
    /// Ready(Continue) or Pending indicates the connection should continue processing messages.
    /// Ready(Disconnect) will close the connection.
    ///
    /// If you return Pending here, you must ensure that the waker will be woken when you
    /// are ready to make more progress.
    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<ReactorStatus> {
        Poll::Ready(ReactorStatus::Continue)
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

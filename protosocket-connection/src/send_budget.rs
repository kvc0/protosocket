use std::collections::VecDeque;

use bytes::Buf;

use crate::Encoder;

/// A bounded lease on a connection's send capacity. Sends encode directly into
/// the connection's send buffer.
pub struct SendBudget<'a, TEncoder: Encoder> {
    encoder: &'a mut TEncoder,
    send_buffer: &'a mut VecDeque<TEncoder::Serialized>,
    remaining: usize,
}

impl<'s, TEncoder: Encoder> SendBudget<'s, TEncoder> {
    pub(crate) fn new(
        encoder: &'s mut TEncoder,
        send_buffer: &'s mut VecDeque<TEncoder::Serialized>,
        remaining: usize,
    ) -> Self {
        Self {
            encoder,
            send_buffer,
            remaining,
        }
    }

    /// Take a permit to send one message. None when the budget is spent.
    /// Dropping a permit unused returns its capacity.
    /// The permit hold &mut on the budget - you must spend or drop it before
    /// you can reserve another.
    ///
    /// You can reserve ahead of doing work to exert backpressure if outbound writes
    /// would be blocked.
    pub fn reserve(&mut self) -> Option<SendPermit<'_, 's, TEncoder>> {
        if self.remaining == 0 {
            None
        } else {
            Some(SendPermit { budget: self })
        }
    }
}

/// A reserved slot in a connection's send queue. A permit is one send: spending it
/// consumes it.
pub struct SendPermit<'a, 'b, TEncoder: Encoder> {
    budget: &'a mut SendBudget<'b, TEncoder>,
}

impl<TEncoder: Encoder> SendPermit<'_, '_, TEncoder> {
    /// Spend the permit to send a message.
    pub fn send(self, message: TEncoder::Message) {
        self.budget.remaining -= 1;
        let buffer = self.budget.encoder.encode(message);
        log::trace!("serialized reactor message: {}b", buffer.remaining());
        self.budget.send_buffer.push_back(buffer);
    }
}

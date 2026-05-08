use std::sync::Arc;

use crate::shared::Shared;

/// The sending half of a Spillway channel.
///
/// You `clone()` to create more Senders.
pub struct Sender<T> {
    chute: usize,
    shared: Arc<Shared<T>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.add_sender();
        Self {
            chute: self.shared.choose_chute(),
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let was = self.shared.drop_sender();
        if was == 1 {
            self.shared.wake();
        }
    }
}

impl<T> std::fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender")
            .field("chute", &self.chute)
            .finish()
    }
}

impl<T> Sender<T> {
    pub(crate) fn new(shared: Arc<Shared<T>>) -> Self {
        shared.add_sender();
        Self {
            chute: shared.choose_chute(),
            shared,
        }
    }

    /// Send a value to the Receiver.
    ///
    /// Messages are only guaranteed to arrive in order on a per-Sender basis.
    ///
    /// If you send 1, 2, 3 in this Sender, you will receive 1, 2, 3 in that order.
    /// If you send 4, 5, 6 from another Sender, you will receive 4, 5, 6 in that order too.
    ///
    /// However, you might receive 1, 4, 5, 2, 3, 6 or any other interleaving. But
    /// 1 will always appear before 2, and 2 before 3; and 4 will always appear before 5,
    /// and 5 before 6.
    pub fn send(&self, value: T) -> Result<(), T> {
        self.shared.send(self.chute, value)
    }

    /// Send a batch of values to the Receiver.
    ///
    /// Messages are only guaranteed to arrive in order on a per-Sender basis.
    ///
    /// `send_many` will result in the batch being sent atomically; once the receiver sees the first
    /// value from the batch, it is guaranteed to see all of the values in the batch before any other
    /// values.
    ///
    /// If you send [1, 2, 3] in this Sender, at the same time as you send [4, 5] and then 6 from
    /// another Sender, each of these orderings (and no others) are valid and possible for the
    /// Receiver to see:
    ///
    /// |      order       |
    /// | ---------------- |
    /// | 1, 2, 3, 4, 5, 6 |
    /// | 4, 5, 1, 2, 3, 6 |
    /// | 4, 5, 6, 1, 2, 3 |
    pub fn send_many<I: IntoIterator<Item = T>>(&self, values: I) -> Result<(), I> {
        self.shared.send_many(self.chute, values)
    }
}

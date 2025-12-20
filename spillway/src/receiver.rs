use std::{collections::VecDeque, sync::Arc};

use crate::shared::Shared;

/// The receiving half of a Spillway channel.
pub struct Receiver<T> {
    cursor: usize,
    buffer: VecDeque<T>,
    shared: Arc<Shared<T>>,
}

impl<T> std::fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver")
            .field("cursor", &self.cursor)
            .finish()
    }
}

impl<T> Receiver<T> {
    /// Create a new receiver with a concurrency
    pub(crate) fn new(shared: Arc<Shared<T>>) -> Self {
        Self {
            cursor: 0,
            buffer: Default::default(),
            shared,
        }
    }

    /// This pattern does check whether the channel is closed. It's useful for examples and
    /// some kinds of synchronous code, but use `next` or `poll_next` with async code.
    pub fn try_next(&mut self) -> Option<T> {
        match self.poll_next(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
            std::task::Poll::Ready(next) => next,
            std::task::Poll::Pending => None,
        }
    }

    /// The raw next value for the Receiver.
    ///
    /// * `Poll::Pending` when caught up and waiting for new messages.
    /// * `Poll::Ready(Some(value))` for the next value.
    /// * `Poll::Ready(None)` when all senders have been dropped and the Receiver is caught up. The Receiver will never receive more messages and you should drop it.
    pub fn poll_next(&mut self, context: &mut std::task::Context) -> std::task::Poll<Option<T>> {
        match self.buffer.pop_front() {
            Some(next) => std::task::Poll::Ready(Some(next)),
            None => {
                let dirty_index = match self.shared.race_find_dirty(self.cursor) {
                    Some(dirty_index) => {
                        log::debug!("found dirty {dirty_index}");
                        dirty_index
                    }
                    None => {
                        // we might park, but we gotta double check first after registering for wake
                        self.shared.waker.register(context.waker());
                        match self.shared.race_find_dirty(self.cursor) {
                            Some(dirty_index) => {
                                log::debug!("found dirty on double-check {dirty_index}");
                                dirty_index
                            }
                            None => {
                                // Well, wait - are we completely done now?
                                if 0 == self
                                    .shared
                                    .senders
                                    .load(std::sync::atomic::Ordering::Relaxed)
                                {
                                    log::debug!("all done receiving and no more senders exist");
                                    return std::task::Poll::Ready(None);
                                }
                                log::trace!("pending: {:#?}", self.shared);
                                return std::task::Poll::Pending;
                            }
                        }
                    }
                };
                // cursor points at a dirty index.
                debug_assert_eq!(0, self.buffer.len());
                self.shared.chutes[dirty_index].swap(&mut self.buffer);
                self.cursor = (dirty_index + 1) % self.shared.chutes.len();
                log::debug!(
                    "buffer: {}, cursor: {}, next: {:?}",
                    self.buffer.len(),
                    self.cursor,
                    self.shared.race_find_dirty(self.cursor)
                );

                let next = self
                    .buffer
                    .pop_front()
                    .expect("chutes are only dirty when they have contents");
                std::task::Poll::Ready(Some(next))
            }
        }
    }

    /// The next value for the Receiver.
    ///
    /// * Some(T) is the next value.
    /// * None when all senders have been dropped and the Receiver is caught up. The Receiver will never receive more messages and you should drop it.
    #[inline]
    pub async fn next(&mut self) -> Option<T> {
        std::future::poll_fn(|context| self.poll_next(context)).await
    }
}

#[cfg(test)]
mod test {
    use std::task::{Context, Poll, Waker};

    use crate::{channel_with_concurrency, Receiver};

    fn poll(receiver: &mut Receiver<i32>) -> Poll<Option<i32>> {
        receiver.poll_next(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn test_channel() {
        let (sender, mut receiver) = channel_with_concurrency(1);
        assert_eq!(1, receiver.shared.chutes.len());
        assert!(receiver.shared.chutes[0].clean());

        assert_eq!(Poll::Pending, poll(&mut receiver));
        assert_eq!(Poll::Pending, poll(&mut receiver));
        sender.send(1).expect("sends");
        assert!(!receiver.shared.chutes[0].clean());
        assert_eq!(Poll::Ready(Some(1)), poll(&mut receiver));
        assert!(receiver.shared.chutes[0].clean());
    }

    #[test]
    fn test_channel_immediate() {
        let (sender, mut receiver) = channel_with_concurrency(1);
        sender.send(1).expect("sends");
        assert_eq!(Poll::Ready(Some(1)), poll(&mut receiver));
    }

    #[test]
    fn test_channel_two() {
        let (sender, mut receiver) = channel_with_concurrency(2);
        let sender2 = sender.clone();
        sender.send(0).expect("sends");
        sender.send(1).expect("sends");
        sender2.send(2).expect("sends");
        sender2.send(3).expect("sends");

        assert!(!receiver.shared.chutes[0].clean());
        assert!(!receiver.shared.chutes[1].clean());

        assert_eq!(Poll::Ready(Some(0)), poll(&mut receiver));

        assert!(receiver.shared.chutes[0].clean());
        assert!(!receiver.shared.chutes[1].clean());

        assert_eq!(Poll::Ready(Some(1)), poll(&mut receiver));

        assert_eq!(Poll::Ready(Some(2)), poll(&mut receiver));
        assert!(receiver.shared.chutes[0].clean());
        assert!(receiver.shared.chutes[1].clean());
        sender.send(4).expect("sends");
        assert!(!receiver.shared.chutes[0].clean());

        assert_eq!(Poll::Ready(Some(3)), poll(&mut receiver));
        assert_eq!(Poll::Ready(Some(4)), poll(&mut receiver));
        assert_eq!(Poll::Pending, poll(&mut receiver));
    }
}

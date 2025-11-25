use std::{collections::VecDeque, sync::{Arc, Mutex, atomic::AtomicUsize}};

use crate::shared::Shared;

pub struct Receiver<T> {
    cursor: usize,
    buffer: VecDeque<T>,
    shared: Arc<Shared<T>>,
}
impl<T> Receiver<T> {
    pub(crate) fn new(shared: Arc<Shared<T>>) -> Self {
        Self {
            cursor: 0,
            buffer: Default::default(),
            shared,
        }
    }

    pub fn poll_next(&mut self, context: &mut std::task::Context) -> std::task::Poll<Option<T>> {
        let next = if self.buffer.is_empty() {
            for _i in 0..self.shared.chutes.len() {
                std::mem::swap(&mut *self.shared.chutes[self.cursor].lock().expect("must not be poisoned"), &mut self.buffer);
                self.cursor = (self.cursor + 1) % self.shared.chutes.len();
                if !self.buffer.is_empty() {
                    break
                }
            }
            if self.buffer.is_empty() {
                self.shared.waker.register(context.waker());
                // gotta do a double-check or else waker can drop liveness events due to toctou
                if self.shared.chutes.iter().all(|chute| chute.lock().expect("must not be poisoned").is_empty()) {
                    // All queues are empty
                    if self.shared.senders.load(std::sync::atomic::Ordering::Acquire) == 0 {
                        // handle toctou for last items...
                        for chute in self.shared.chutes.iter() {
                            self.buffer.extend(chute.lock().expect("must not be poisoned").drain(..));
                        }
                        if self.buffer.is_empty() {
                            return std::task::Poll::Ready(None);
                        } else {
                            // we'll come back around
                            self.shared.wake();
                        }
                    }
                } else {
                    // There's some stuff in a channel, from racing
                    self.shared.wake();
                }
                return std::task::Poll::Pending
            }
            self.buffer.pop_front().expect("already checked")
        } else {
            self.buffer.pop_front().expect("already checked")
        };

        std::task::Poll::Ready(Some(next))
    }

    #[inline]
    pub async fn next(&mut self) -> Option<T> {
        std::future::poll_fn(|context| self.poll_next(context)).await
    }
}

use std::{
    future::Future,
    pin::{pin, Pin},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use futures::{task::AtomicWaker, Stream};

#[derive(Debug)]
pub struct IdentifiableAbortable<F> {
    f: F,
    aborted: Arc<AtomicUsize>,
    waker: Arc<AtomicWaker>,
}

impl<F> IdentifiableAbortable<F> {
    pub fn new(f: F) -> (Self, IdentifiableAbortHandle) {
        let aborted = Arc::new(AtomicUsize::new(0));
        let waker = Arc::new(AtomicWaker::new());
        (
            Self {
                f,
                aborted: aborted.clone(),
                waker: waker.clone(),
            },
            IdentifiableAbortHandle { aborted, waker },
        )
    }
}

#[derive(Debug)]
pub enum AbortableState<T> {
    Abort,
    Aborted,
    Ready(T),
}

impl<F> Future for IdentifiableAbortable<F>
where
    F: Future,
{
    type Output = AbortableState<crate::Result<F::Output>>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let state = self.aborted.load(Ordering::Relaxed);
        if 1 == state {
            self.aborted.store(2, Ordering::Relaxed);
            return Poll::Ready(AbortableState::Abort);
        }
        if 2 == state {
            return Poll::Ready(AbortableState::Aborted);
        }
        self.waker.register(context.waker());
        // SAFETY: This is a structural pin. If I'm not moved then neither is this future.
        let structurally_pinned_future = unsafe { self.as_mut().map_unchecked_mut(|me| &mut me.f) };
        structurally_pinned_future
            .poll(context)
            .map(|output| AbortableState::Ready(Ok(output)))
    }
}

impl<S> Stream for IdentifiableAbortable<S>
where
    S: Stream + Unpin,
{
    type Item = AbortableState<crate::Result<S::Item>>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.waker.register(context.waker());
        match self.aborted.load(Ordering::Relaxed) {
            0 => {
                match pin!(&mut self.f).poll_next(context) {
                    Poll::Ready(next) => {
                        match next {
                            Some(next) => Poll::Ready(Some(AbortableState::Ready(Ok(next)))),
                            None => {
                                // stream is done
                                self.aborted.store(3, Ordering::Relaxed);
                                Poll::Ready(Some(AbortableState::Ready(Err(
                                    crate::Error::Finished,
                                ))))
                            }
                        }
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            1 => {
                self.aborted.store(2, Ordering::Relaxed);
                Poll::Ready(Some(AbortableState::Abort))
            }
            2 => {
                self.aborted.store(3, Ordering::Relaxed);
                Poll::Ready(Some(AbortableState::Aborted))
            }
            _ => Poll::Ready(None),
        }
    }
}

#[derive(Debug)]
pub struct IdentifiableAbortHandle {
    aborted: Arc<AtomicUsize>,
    waker: Arc<AtomicWaker>,
}
impl IdentifiableAbortHandle {
    /// Mark the future or stream as externally cancelled - don't send a cancellation
    pub fn mark_aborted(self) {
        self.aborted.store(2, Ordering::Relaxed);
        self.waker.wake();
    }
}

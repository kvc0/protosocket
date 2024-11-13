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

impl<F> Unpin for IdentifiableAbortable<F> {}

pub struct IdentifiableAbortable<F> {
    f: F,
    aborted: Arc<AtomicUsize>,
    waker: Arc<AtomicWaker>,
    id: u64,
}

impl<F> IdentifiableAbortable<F> {
    pub fn new(id: u64, f: F) -> (Self, IdentifiableAbortHandle) {
        let aborted = Arc::new(AtomicUsize::new(0));
        let waker = Arc::new(AtomicWaker::new());
        (
            Self {
                f,
                aborted: aborted.clone(),
                waker: waker.clone(),
                id,
            },
            IdentifiableAbortHandle { aborted, waker },
        )
    }
}

impl<F> Future for IdentifiableAbortable<F>
where
    F: Future + Unpin,
{
    type Output = (u64, Option<F::Output>);

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if 1 == self.aborted.load(Ordering::Relaxed) {
            return Poll::Ready((self.id, None));
        }
        self.waker.register(context.waker());
        pin!(&mut self.f)
            .poll(context)
            .map(|output| (self.id, Some(output)))
    }
}

impl<S> Stream for IdentifiableAbortable<S>
where
    S: Stream + Unpin,
{
    type Item = (u64, Option<S::Item>);

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.aborted.load(Ordering::Relaxed) {
            1 => {
                self.aborted.store(2, Ordering::Relaxed);
                return Poll::Ready(Some((self.id, None)));
            }
            2 => return Poll::Ready(None),
            _ => {}
        }
        match pin!(&mut self.f).poll_next(context) {
            Poll::Ready(next) => {
                match next {
                    Some(next) => Poll::Ready(Some((self.id, Some(next)))),
                    None => {
                        // stream is done
                        self.aborted.store(2, Ordering::Relaxed);
                        Poll::Ready(Some((self.id, None)))
                    }
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct IdentifiableAbortHandle {
    aborted: Arc<AtomicUsize>,
    waker: Arc<AtomicWaker>,
}
impl IdentifiableAbortHandle {
    pub fn abort(&self) {
        let _ = self
            .aborted
            .compare_exchange(0, 1, Ordering::Release, Ordering::Relaxed);
        self.waker.wake();
    }
}

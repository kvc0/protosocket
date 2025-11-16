use std::{
    future::Future,
    pin::Pin,
    sync::{atomic::AtomicUsize, Arc},
    task::{Context, Poll},
};

pub(crate) struct ConcurrencyTracker<F> {
    concurrency: Arc<AtomicUsize>,
    future: F,
}
impl<F> ConcurrencyTracker<F> {
    pub fn wrap(concurrency: Arc<AtomicUsize>, future: F) -> Self {
        concurrency.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            concurrency,
            future,
        }
    }
}
impl<F> Future for ConcurrencyTracker<F>
where
    F: Future,
{
    type Output = F::Output;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // This is okay because `field` is pinned when `self` is.
        // https://doc.rust-lang.org/std/pin/index.html#choosing-pinning-to-be-structural-for-field
        unsafe { self.map_unchecked_mut(|s| &mut s.future) }.poll(context)
    }
}
impl<F> Drop for ConcurrencyTracker<F> {
    fn drop(&mut self) {
        self.concurrency
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

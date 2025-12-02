use std::{future::Future, pin::Pin, sync::Arc, task::{Context, Poll}};

use crate::{Message, server::{abortable::AbortableState, abortion_tracker::AbortionTracker}};


pub struct ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    future: F,
    id: u64,
    forward: spillway::Sender<T>,
    aborts: Arc<AbortionTracker>,
}
impl<F, T> Drop for ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    fn drop(&mut self) {
        self.aborts.take_abort(self.id);
    }
}
impl<F, T> ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    pub fn new(future: F, id: u64, forward: spillway::Sender<T>, aborts: Arc<AbortionTracker>) -> Self {
        Self {
            future,
            id,
            forward,
            aborts,
        }
    }
}
impl<F, T> Future for ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    type Output = ();
    
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: This is a structural pin. If I'm not moved then neither is this future.
        let structurally_pinned_future =
            unsafe { self.as_mut().map_unchecked_mut(|me| &mut me.future) };
        match structurally_pinned_future.poll(context) {
            Poll::Ready(state) => {
                let abort = self.aborts.take_abort(self.id);
                match state {
                    AbortableState::Ready(Ok(response)) => {
                        log::trace!("{} unary rpc response", self.id);
                        if let Err(_e) = self.forward.send(response) {
                            log::debug!("outbound connection is closed");
                        }
                        Poll::Ready(())
                    }
                    AbortableState::Ready(Err(e)) => {
                        match e {
                            crate::Error::IoFailure(error) => {
                                log::warn!("{} io failure while servicing rpc: {error:?}", self.id);
                                if let Some(abort) = abort {
                                    abort.abort();
                                }
                            }
                            crate::Error::CancelledRemotely => {
                                log::debug!("{} rpc cancelled remotely", self.id);
                                if let Some(abort) = abort {
                                    abort.abort();
                                }
                            }
                            crate::Error::ConnectionIsClosed => {
                                log::debug!("{} rpc cancelled remotely", self.id);
                                if let Some(abort) = abort {
                                    abort.abort();
                                }
                            }
                            crate::Error::Finished => {
                                log::debug!("{} unary rpc ended", self.id);
                                if let Some(abort) = abort {
                                    if let Err(_e) = self.forward.send(
                                        T::ended(self.id),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                    }
                                    abort.mark_aborted();
                                }
                            }
                        }
                        Poll::Ready(())
                        // cancelled
                    }
                    AbortableState::Abort => {
                        // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                        log::debug!("{} unary rpc abort", self.id);
                        if let Some(abort) = abort {
                            abort.abort();
                        }
                        Poll::Ready(())
                    }
                    AbortableState::Aborted => {
                        // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                        log::debug!("{} unary rpc done", self.id);
                        if let Some(abort) = abort {
                            abort.mark_aborted();
                        }
                        Poll::Ready(())
                    }
                }
            }
            Poll::Pending => {
                Poll::Pending
            }
        }
    }
}

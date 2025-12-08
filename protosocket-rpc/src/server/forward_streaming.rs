use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;

use crate::{server::abortable::AbortableState, Message};

pub struct ForwardAbortableStreamingRpc<S, T>
where
    S: Stream<Item = AbortableState<crate::Result<T>>>,
    T: Message,
{
    stream: S,
    id: u64,
    forward: spillway::Sender<T>,
    completed_for_drop: bool,
}
impl<S, T> Drop for ForwardAbortableStreamingRpc<S, T>
where
    S: Stream<Item = AbortableState<crate::Result<T>>>,
    T: Message,
{
    fn drop(&mut self) {
        if !self.completed_for_drop {
            log::debug!("dropping unary rpc before completion: {}", self.id);
            let _ = self.forward.send(T::cancelled(self.id));
        }
    }
}
impl<S, T> ForwardAbortableStreamingRpc<S, T>
where
    S: Stream<Item = AbortableState<crate::Result<T>>>,
    T: Message,
{
    pub fn new(stream: S, id: u64, forward: spillway::Sender<T>) -> Self {
        Self {
            stream,
            id,
            forward,
            completed_for_drop: false,
        }
    }

    fn complete_for_drop(self: Pin<&mut Self>) {
        // SAFETY: This is a structural pin. If I'm not moved then neither is this boolean (it was an invariant for the future anyway).
        unsafe {
            self.get_unchecked_mut().completed_for_drop = true;
        }
    }
}
impl<S, T> Future for ForwardAbortableStreamingRpc<S, T>
where
    S: Stream<Item = AbortableState<crate::Result<T>>>,
    T: Message,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            // SAFETY: This is a structural pin. If I'm not moved then neither is this future.
            break match unsafe { self.as_mut().map_unchecked_mut(|me| &mut me.stream) }
                .poll_next(context)
            {
                Poll::Ready(state) => {
                    match state {
                        Some(AbortableState::Ready(Ok(response))) => {
                            log::trace!("{} unary rpc response", self.id);
                            if let Err(_e) = self.forward.send(response) {
                                log::debug!("outbound connection is closed");
                            }
                            continue;
                        }
                        Some(AbortableState::Ready(Err(e))) => {
                            match e {
                                crate::Error::IoFailure(error) => {
                                    log::warn!(
                                        "{} io failure while servicing rpc: {error:?}",
                                        self.id
                                    );
                                    let _ = self.forward.send(T::cancelled(self.id));
                                }
                                crate::Error::CancelledRemotely => {
                                    log::debug!("{} rpc cancelled remotely", self.id);
                                    let _ = self.forward.send(T::cancelled(self.id));
                                }
                                crate::Error::ConnectionIsClosed => {
                                    log::debug!("{} rpc cancelled remotely", self.id);
                                    let _ = self.forward.send(T::cancelled(self.id));
                                }
                                crate::Error::Finished => {
                                    log::debug!("{} unary rpc ended", self.id);
                                    if let Err(_e) = self.forward.send(T::ended(self.id)) {
                                        log::debug!("outbound connection is closed");
                                    }
                                }
                            }
                            self.complete_for_drop();
                            Poll::Ready(())
                        }
                        Some(AbortableState::Abort) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{} unary rpc abort", self.id);
                            let _ = self.forward.send(T::cancelled(self.id));
                            self.complete_for_drop();
                            Poll::Ready(())
                        }
                        Some(AbortableState::Aborted) => {
                            log::debug!("{} unary rpc was cancelled by the client.", self.id);
                            let _ = self.forward.send(T::cancelled(self.id));
                            self.complete_for_drop();
                            Poll::Ready(())
                        }
                        None => {
                            log::debug!("{} streaming rpc reached the end", self.id);
                            let _ = self.forward.send(T::ended(self.id));
                            self.complete_for_drop();
                            Poll::Ready(())
                        }
                    }
                }
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

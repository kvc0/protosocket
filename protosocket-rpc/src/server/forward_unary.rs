use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{
    server::{abortable::AbortableState, rpc_submitter::RpcResponse},
    Message,
};

pub struct ForwardAbortableUnaryRpc<F, T>
where
    F: Future<Output = AbortableState<crate::Result<T>>>,
    T: Message,
{
    future: F,
    id: u64,
    forward: spillway::Sender<RpcResponse<T>>,
    completed_for_drop: bool,
}
impl<F, T> Drop for ForwardAbortableUnaryRpc<F, T>
where
    F: Future<Output = AbortableState<crate::Result<T>>>,
    T: Message,
{
    fn drop(&mut self) {
        if !self.completed_for_drop {
            log::debug!("dropping unary rpc before completion: {}", self.id);
            let _ = self.forward.send(RpcResponse::Final(T::cancelled(self.id)));
        }
    }
}
impl<F, T> ForwardAbortableUnaryRpc<F, T>
where
    F: Future<Output = AbortableState<crate::Result<T>>>,
    T: Message,
{
    pub fn new(future: F, id: u64, forward: spillway::Sender<RpcResponse<T>>) -> Self {
        Self {
            future,
            id,
            forward,
            completed_for_drop: false,
        }
    }
}
impl<F, T> Future for ForwardAbortableUnaryRpc<F, T>
where
    F: Future<Output = AbortableState<crate::Result<T>>>,
    T: Message,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: This is a structural pin. If I'm not moved then neither is this future.
        let structurally_pinned_future =
            unsafe { self.as_mut().map_unchecked_mut(|me| &mut me.future) };
        match structurally_pinned_future.poll(context) {
            Poll::Ready(state) => {
                // SAFETY: This is a structural pin. If I'm not moved then neither is this boolean (it was an invariant for the future anyway).
                unsafe {
                    self.as_mut().get_unchecked_mut().completed_for_drop = true;
                }

                match state {
                    AbortableState::Ready(Ok(response)) => {
                        log::trace!("{} unary rpc response", self.id);
                        if let Err(_e) = self.forward.send(RpcResponse::Final(response)) {
                            log::debug!("outbound connection is closed");
                        }
                        Poll::Ready(())
                    }
                    AbortableState::Ready(Err(e)) => {
                        match e {
                            crate::Error::IoFailure(error) => {
                                log::warn!("{} io failure while servicing rpc: {error:?}", self.id);
                                let _ =
                                    self.forward.send(RpcResponse::Final(T::cancelled(self.id)));
                            }
                            crate::Error::CancelledRemotely => {
                                log::debug!("{} rpc cancelled remotely", self.id);
                                let _ =
                                    self.forward.send(RpcResponse::Final(T::cancelled(self.id)));
                            }
                            crate::Error::ConnectionIsClosed => {
                                log::debug!("{} rpc cancelled remotely", self.id);
                                let _ =
                                    self.forward.send(RpcResponse::Final(T::cancelled(self.id)));
                            }
                            crate::Error::Finished => {
                                log::debug!("{} unary rpc ended", self.id);
                                if let Err(_e) =
                                    self.forward.send(RpcResponse::Final(T::ended(self.id)))
                                {
                                    log::debug!("outbound connection is closed");
                                }
                            }
                        }
                        Poll::Ready(())
                        // cancelled
                    }
                    AbortableState::Abort => {
                        // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                        log::debug!("{} unary rpc abort", self.id);
                        let _ = self.forward.send(RpcResponse::Final(T::cancelled(self.id)));
                        Poll::Ready(())
                    }
                    AbortableState::Aborted => {
                        log::debug!("{} unary rpc was cancelled by the client.", self.id);
                        let _ = self.forward.send(RpcResponse::Final(T::cancelled(self.id)));
                        Poll::Ready(())
                    }
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

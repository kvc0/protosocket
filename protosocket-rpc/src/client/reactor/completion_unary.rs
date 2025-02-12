use std::{
    future::Future,
    pin::{pin, Pin},
    task::{Context, Poll},
};

use tokio::sync::oneshot;

use super::completion_registry::CompletionGuard;
use crate::Message;

/// A completion for a unary RPC.
#[derive(Debug)]
pub struct UnaryCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
    completion: oneshot::Receiver<crate::Result<Response>>,
    completion_guard: CompletionGuard<Response, Request>,
}

impl<Response, Request> UnaryCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
    pub(crate) fn new(
        completion: oneshot::Receiver<crate::Result<Response>>,
        completion_guard: CompletionGuard<Response, Request>,
    ) -> Self {
        Self {
            completion,
            completion_guard,
        }
    }
}

impl<Response, Request> Future for UnaryCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
    type Output = crate::Result<Response>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match pin!(&mut self.completion).poll(context) {
            Poll::Ready(result) => {
                self.completion_guard.set_closed();
                match result {
                    Ok(done) => Poll::Ready(done),
                    Err(_cancelled) => Poll::Ready(Err(crate::Error::CancelledRemotely)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

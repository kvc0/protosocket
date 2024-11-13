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
pub struct UnaryCompletion<Response>
where
    Response: Message,
{
    completion: oneshot::Receiver<crate::Result<Response>>,
    _completion_guard: CompletionGuard<Response>,
}

impl<Response> UnaryCompletion<Response>
where
    Response: Message,
{
    pub(crate) fn new(
        completion: oneshot::Receiver<crate::Result<Response>>,
        completion_guard: CompletionGuard<Response>,
    ) -> Self {
        Self {
            completion,
            _completion_guard: completion_guard,
        }
    }
}

impl<Response> Future for UnaryCompletion<Response>
where
    Response: Message,
{
    type Output = crate::Result<Response>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match pin!(&mut self.completion).poll(context) {
            Poll::Ready(result) => match result {
                Ok(done) => Poll::Ready(done),
                Err(_cancelled) => Poll::Ready(Err(crate::Error::CancelledRemotely)),
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use super::completion_registry::CompletionGuard;
use crate::Message;

/// A completion for a streaming RPC.
///
/// Make sure you process this stream quickly, and drop data yourself if you have to. The
/// server will send data as quickly as it can.
#[derive(Debug)]
pub struct StreamingCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
    completion: spillway::Receiver<Response>,
    completion_guard: CompletionGuard<Response, Request>,
    closed: bool,
}

/// SAFETY: There is no unsafe code in this implementation
impl<Response, Request> Unpin for StreamingCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
}

impl<Response, Request> StreamingCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
    pub(crate) fn new(
        completion: spillway::Receiver<Response>,
        completion_guard: CompletionGuard<Response, Request>,
    ) -> Self {
        Self {
            completion,
            completion_guard,
            closed: false,
        }
    }
}

impl<Response, Request> futures::Stream for StreamingCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
    type Item = crate::Result<Response>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.closed {
            return Poll::Ready(None);
        }
        match self.completion.poll_next(context) {
            Poll::Ready(Some(next)) => Poll::Ready(Some(Ok(next))),
            Poll::Ready(None) => {
                self.closed = true;
                self.completion_guard.set_closed();
                Poll::Ready(Some(Err(crate::Error::Finished)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

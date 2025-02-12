use std::{
    pin::{pin, Pin},
    task::{Context, Poll},
};

use tokio::sync::mpsc;

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
    completion: mpsc::UnboundedReceiver<Response>,
    completion_guard: CompletionGuard<Response, Request>,
    closed: bool,
    nexts: Vec<Response>,
}

/// SAFETY: There is no unsafe code in this implementation
impl<Response, Request> Unpin for StreamingCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
}

const LIMIT: usize = 16;

impl<Response, Request> StreamingCompletion<Response, Request>
where
    Response: Message,
    Request: Message,
{
    pub(crate) fn new(
        completion: mpsc::UnboundedReceiver<Response>,
        completion_guard: CompletionGuard<Response, Request>,
    ) -> Self {
        Self {
            completion,
            completion_guard,
            closed: false,
            nexts: Vec::with_capacity(LIMIT),
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
        if self.nexts.is_empty() {
            let Self {
                completion, nexts, ..
            } = &mut *self;
            let received = pin!(completion).poll_recv_many(context, nexts, LIMIT);
            match received {
                Poll::Ready(count) => {
                    if count == 0 {
                        self.closed = true;
                        self.completion_guard.set_closed();
                        return Poll::Ready(Some(Err(crate::Error::Finished)));
                    }
                    // because it is a vector, we have to consume in reverse order. This is because
                    // of the poll_recv_many argument type.
                    nexts.reverse();
                }
                Poll::Pending => return Poll::Pending,
            }
        }
        match self.nexts.pop() {
            Some(next) => Poll::Ready(Some(Ok(next))),
            None => {
                log::error!("unexpected empty nexts");
                Poll::Ready(None)
            }
        }
    }
}

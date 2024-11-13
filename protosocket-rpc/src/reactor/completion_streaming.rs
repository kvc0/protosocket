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
/// server will send data as quickly as it can. To avoid needless framework buffers in the
/// intended use case, protosockets do not buffer RPC response data internally beyond what
/// the network does.
#[derive(Debug)]
pub struct StreamingCompletion<Response>
where
    Response: Message,
{
    completion: mpsc::UnboundedReceiver<Response>,
    _completion_guard: CompletionGuard<Response>,
    closed: bool,
    nexts: Vec<Response>,
}

/// SAFETY: There is no unsafe code in this implementation
impl<Response> Unpin for StreamingCompletion<Response> where Response: Message {}

const LIMIT: usize = 16;

impl<Response> StreamingCompletion<Response>
where
    Response: Message,
{
    pub(crate) fn new(
        completion: mpsc::UnboundedReceiver<Response>,
        completion_guard: CompletionGuard<Response>,
    ) -> Self {
        Self {
            completion,
            _completion_guard: completion_guard,
            closed: false,
            nexts: Vec::with_capacity(LIMIT),
        }
    }
}

impl<Response> futures::Stream for StreamingCompletion<Response>
where
    Response: Message,
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
                        return Poll::Ready(Some(Err(crate::Error::Finished)));
                    }
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

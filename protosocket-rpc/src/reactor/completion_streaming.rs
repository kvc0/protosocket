use std::{
    pin::{pin, Pin},
    task::{Context, Poll},
};

use tokio::sync::mpsc;

use super::rpc_drop_guard::RpcDropGuard;
use crate::Message;

/// A completion for a streaming RPC.
///
/// Make sure you process this stream quickly, and drop data yourself if you have to. The
/// server will send data as quickly as it can. To avoid needless framework buffers in the
/// intended use case, protosockets do not buffer RPC response data internally beyond what
/// the network does.
#[derive(Debug)]
pub struct StreamingCompletion<Request, Response>
where
    Request: Message,
    Response: Message,
{
    completion: mpsc::UnboundedReceiver<Response>,
    drop_guard: RpcDropGuard<Request>,
    closed: bool,
    nexts: Vec<Response>,
}

/// SAFETY: There is no unsafe code in this implementation
impl<Request, Response> Unpin for StreamingCompletion<Request, Response>
where
    Request: Message,
    Response: Message,
{
}

const LIMIT: usize = 16;

impl<Request, Response> StreamingCompletion<Request, Response>
where
    Request: Message,
    Response: Message,
{
    pub fn new(
        request_id: u64,
        cancellation_submission_queue: mpsc::Sender<Request>,
    ) -> (mpsc::UnboundedSender<Response>, Self) {
        let (completor, completion) = mpsc::unbounded_channel();
        (
            completor,
            Self {
                completion,
                drop_guard: RpcDropGuard::new(cancellation_submission_queue, request_id),
                closed: false,
                nexts: Vec::with_capacity(LIMIT),
            },
        )
    }
}

impl<Request, Response> futures::Stream for StreamingCompletion<Request, Response>
where
    Request: Message,
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
                        self.drop_guard.set_complete();
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

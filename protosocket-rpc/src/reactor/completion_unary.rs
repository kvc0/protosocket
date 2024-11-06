use std::{
    future::Future,
    pin::{pin, Pin},
    task::{Context, Poll},
};

use tokio::sync::{mpsc, oneshot};

use super::rpc_drop_guard::RpcDropGuard;
use crate::Message;

/// A completion for a unary RPC.
#[derive(Debug)]
pub struct UnaryCompletion<Request, Response>
where
    Request: Message,
    Response: Message,
{
    completion: oneshot::Receiver<crate::Result<Response>>,
    drop_guard: RpcDropGuard<Request>,
}

impl<Request, Response> UnaryCompletion<Request, Response>
where
    Request: Message,
    Response: Message,
{
    pub fn new(
        request_id: u64,
        cancellation_submission_queue: mpsc::Sender<Request>,
    ) -> (oneshot::Sender<crate::Result<Response>>, Self) {
        let (completor, completion) = oneshot::channel();
        (
            completor,
            Self {
                completion,
                drop_guard: RpcDropGuard::new(cancellation_submission_queue, request_id),
            },
        )
    }
}

impl<Request, Response> Future for UnaryCompletion<Request, Response>
where
    Request: Message,
    Response: Message,
{
    type Output = crate::Result<Response>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match pin!(&mut self.completion).poll(context) {
            Poll::Ready(result) => match result {
                Ok(done) => {
                    self.drop_guard.set_complete();
                    Poll::Ready(done)
                }
                Err(_cancelled) => {
                    self.drop_guard.set_complete();
                    Poll::Ready(Err(crate::Error::CancelledRemotely))
                }
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

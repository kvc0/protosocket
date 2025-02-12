use std::sync::{atomic::AtomicBool, Arc};

use tokio::sync::{mpsc, oneshot};

use super::reactor::completion_reactor::{DoNothingMessageHandler, RpcCompletionReactor};
use super::reactor::completion_registry::{Completion, CompletionGuard, RpcRegistrar};
use super::reactor::{
    completion_streaming::StreamingCompletion, completion_unary::UnaryCompletion,
};
use crate::Message;

/// A client for sending RPCs to a protosockets rpc server.
///
/// It handles sending messages to the server and associating the responses.
/// Messages are sent and received in any order, asynchronously, and support cancellation.
/// To cancel an RPC, drop the response future.
#[derive(Debug, Clone)]
pub struct RpcClient<Request, Response>
where
    Request: Message,
    Response: Message,
{
    #[allow(clippy::type_complexity)]
    in_flight_submission: RpcRegistrar<Response>,
    submission_queue: tokio::sync::mpsc::Sender<Request>,
    is_alive: Arc<AtomicBool>,
}

impl<Request, Response> RpcClient<Request, Response>
where
    Request: Message,
    Response: Message,
{
    pub(crate) fn new(
        submission_queue: mpsc::Sender<Request>,
        message_reactor: &RpcCompletionReactor<Response, DoNothingMessageHandler<Response>>,
    ) -> Self {
        Self {
            submission_queue,
            in_flight_submission: message_reactor.in_flight_submission_handle(),
            is_alive: message_reactor.alive_handle(),
        }
    }

    /// Checking this before using the client does not guarantee that the client is still alive when you send  
    /// your message. It may be useful for connection pool implementations - for example, [bb8::ManageConnection](https://github.com/djc/bb8/blob/09a043c001b3c15514d9f03991cfc87f7118a000/bb8/src/api.rs#L383-L384)'s  
    /// is_valid and has_broken could be bound to this function to help the pool cycle out broken connections.  
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Send a server-streaming rpc to the server.
    ///
    /// This function only sends the request. You must consume the completion stream to get the response.
    #[must_use = "You must await the completion to get the response. If you drop the completion, the request will be cancelled."]
    pub async fn send_streaming(
        &self,
        request: Request,
    ) -> crate::Result<StreamingCompletion<Response, Request>> {
        let (sender, completion) = mpsc::unbounded_channel();
        let completion_guard = self
            .send_message(Completion::RemoteStreaming(sender), request)
            .await?;

        let completion = StreamingCompletion::new(completion, completion_guard);

        Ok(completion)
    }

    /// Send a unary rpc to the server.
    ///
    /// This function only sends the request. You must await the completion to get the response.
    #[must_use = "You must await the completion to get the response. If you drop the completion, the request will be cancelled."]
    pub async fn send_unary(
        &self,
        request: Request,
    ) -> crate::Result<UnaryCompletion<Response, Request>> {
        let (completor, completion) = oneshot::channel();
        let completion_guard = self
            .send_message(Completion::Unary(completor), request)
            .await?;

        let completion = UnaryCompletion::new(completion, completion_guard);

        Ok(completion)
    }

    async fn send_message(
        &self,
        completion: Completion<Response>,
        request: Request,
    ) -> crate::Result<CompletionGuard<Response, Request>> {
        if !self.is_alive.load(std::sync::atomic::Ordering::Relaxed) {
            // early-out if the connection is closed
            return Err(crate::Error::ConnectionIsClosed);
        }
        let completion_guard = self.in_flight_submission.register_completion(
            request.message_id(),
            completion,
            self.submission_queue.clone(),
        );
        self.submission_queue
            .send(request)
            .await
            .map_err(|_e| crate::Error::ConnectionIsClosed)
            .map(|_| completion_guard)
    }
}

#[cfg(test)]
mod test {
    use std::future::Future;
    use std::pin::pin;
    use std::task::Context;
    use std::task::Poll;

    use futures::task::noop_waker_ref;

    use crate::client::reactor::completion_reactor::DoNothingMessageHandler;
    use crate::client::reactor::completion_reactor::RpcCompletionReactor;
    use crate::Message;

    use super::RpcClient;

    impl Message for u64 {
        fn message_id(&self) -> u64 {
            *self & 0xffffffff
        }

        fn control_code(&self) -> crate::ProtosocketControlCode {
            match *self >> 32 {
                0 => crate::ProtosocketControlCode::Normal,
                1 => crate::ProtosocketControlCode::Cancel,
                2 => crate::ProtosocketControlCode::End,
                _ => unreachable!("invalid control code"),
            }
        }

        fn set_message_id(&mut self, message_id: u64) {
            *self = (*self & 0xf00000000) | message_id;
        }

        fn cancelled(message_id: u64) -> Self {
            (1_u64 << 32) | message_id
        }

        fn ended(message_id: u64) -> Self {
            (2 << 32) | message_id
        }
    }

    fn drive_future<F: Future>(f: F) -> F::Output {
        let mut f = pin!(f);
        loop {
            let next = f.as_mut().poll(&mut Context::from_waker(noop_waker_ref()));
            if let Poll::Ready(result) = next {
                break result;
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn get_client() -> (
        tokio::sync::mpsc::Receiver<u64>,
        RpcClient<u64, u64>,
        RpcCompletionReactor<u64, DoNothingMessageHandler<u64>>,
    ) {
        let (sender, remote_end) = tokio::sync::mpsc::channel::<u64>(10);
        let rpc_reactor = RpcCompletionReactor::<u64, _>::new(DoNothingMessageHandler::default());
        let client = RpcClient::new(sender, &rpc_reactor);
        (remote_end, client, rpc_reactor)
    }

    #[test]
    fn unary_drop_cancel() {
        let (mut remote_end, client, _reactor) = get_client();

        let response = drive_future(client.send_unary(4)).expect("can send");
        assert_eq!(4, remote_end.blocking_recv().expect("a request is sent"));
        assert!(remote_end.is_empty(), "no more messages yet");

        drop(response);

        assert_eq!(
            (1 << 32) + 4,
            remote_end.blocking_recv().expect("a cancel is sent")
        );
    }

    #[test]
    fn streaming_drop_cancel() {
        let (mut remote_end, client, _reactor) = get_client();

        let response = drive_future(client.send_streaming(4)).expect("can send");
        assert_eq!(4, remote_end.blocking_recv().expect("a request is sent"));
        assert!(remote_end.is_empty(), "no more messages yet");

        drop(response);

        assert_eq!(
            (1 << 32) + 4,
            remote_end.blocking_recv().expect("a cancel is sent")
        );
    }
}

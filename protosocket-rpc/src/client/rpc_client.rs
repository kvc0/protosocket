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
    ) -> crate::Result<StreamingCompletion<Response>> {
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
    pub async fn send_unary(&self, request: Request) -> crate::Result<UnaryCompletion<Response>> {
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
    ) -> crate::Result<CompletionGuard<Response>> {
        if !self.is_alive.load(std::sync::atomic::Ordering::Relaxed) {
            // early-out if the connection is closed
            return Err(crate::Error::ConnectionIsClosed);
        }
        let completion_guard = self
            .in_flight_submission
            .register_completion(request.message_id(), completion);
        self.submission_queue
            .send(request)
            .await
            .map_err(|_e| crate::Error::ConnectionIsClosed)
            .map(|_| completion_guard)
    }
}

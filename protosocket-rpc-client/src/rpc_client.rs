use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
};

use k_lock::Mutex;
use tokio::sync::mpsc;

use crate::{
    completion_reactor::{Completion, RpcCompletionReactor},
    completion_streaming::StreamingCompletion,
    completion_unary::UnaryCompletion,
    Message,
};

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
    in_flight_submission: Arc<Mutex<HashMap<u64, Completion<Response>>>>,
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
        message_reactor: &RpcCompletionReactor<Response>,
    ) -> Self {
        Self {
            submission_queue,
            in_flight_submission: message_reactor.in_flight_submission_handle(),
            is_alive: message_reactor.alive_handle(),
        }
    }

    /// Send a server-streaming rpc to the server.
    ///
    /// This function only sends the request. You must consume the completion stream to get the response.
    #[must_use = "You must await the completion to get the response. If you drop the completion, the request will be cancelled."]
    pub async fn send_streaming(
        &self,
        request: Request,
    ) -> crate::Result<StreamingCompletion<Request, Response>> {
        let (sender, completion) =
            StreamingCompletion::new(request.message_id(), self.submission_queue.clone());
        self.send_message(Completion::Streaming(sender), request)
            .await?;

        Ok(completion)
    }

    /// Send a unary rpc to the server.
    ///
    /// This function only sends the request. You must await the completion to get the response.
    #[must_use = "You must await the completion to get the response. If you drop the completion, the request will be cancelled."]
    pub async fn send_unary(
        &self,
        request: Request,
    ) -> crate::Result<UnaryCompletion<Request, Response>> {
        let (sender, completion) =
            UnaryCompletion::new(request.message_id(), self.submission_queue.clone());

        self.send_message(Completion::Unary(sender), request)
            .await?;

        Ok(completion)
    }

    async fn send_message(
        &self,
        completion: Completion<Response>,
        request: Request,
    ) -> crate::Result<()> {
        if !self.is_alive.load(std::sync::atomic::Ordering::Relaxed) {
            // early-out if the connection is closed
            return Err(crate::Error::ConnectionIsClosed);
        }
        self.register_completion(request.message_id(), completion);
        self.submission_queue
            .send(request)
            .await
            .map_err(|_e| crate::Error::ConnectionIsClosed)
    }

    // The triple-buffered message queue mutex is carefully controlled - it can't panic unless the memory allocator panics.
    // Probably the server should crash if that happens.
    // Note that this is just for tracking - you have to register the completion before sending the message, or else you might
    // miss the completion.
    #[allow(clippy::expect_used)]
    fn register_completion(&self, message_id: u64, completion: Completion<Response>) {
        self.in_flight_submission
            .lock()
            .expect("brief internal mutex must work")
            .insert(message_id, completion);
    }
}

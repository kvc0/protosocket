use std::future::Future;

use crate::{
    server::{
        abortable::IdentifiableAbortable, abortion_tracker::AbortionTracker,
        forward_streaming::ForwardAbortableStreamingRpc, forward_unary::ForwardAbortableUnaryRpc,
        rpc_submitter::RpcResponse,
    },
    Message,
};

/// A request context's temporary lease to an RPC Reactor's state.
/// You want to consume your RpcResponder as quickly as possible.
#[must_use]
pub struct RpcResponder<'a, Response> {
    outbound: &'a spillway::Sender<RpcResponse<Response>>,
    aborts: &'a mut AbortionTracker,
    message_id: u64,
}
impl<'a, Response> RpcResponder<'a, Response>
where
    Response: Message,
{
    pub(crate) fn new_responder_reference(
        outbound: &'a spillway::Sender<RpcResponse<Response>>,
        aborts: &'a mut AbortionTracker,
        message_id: u64,
    ) -> Self {
        Self {
            outbound,
            aborts,
            message_id,
        }
    }

    /// Consume the responder by providing a future that will materialize the response to this request.
    pub fn unary(self, unary_rpc: impl Future<Output = Response>) -> impl Future<Output = ()> {
        let (abortable, abort) = IdentifiableAbortable::new(unary_rpc);
        self.aborts.register(self.message_id, abort);
        ForwardAbortableUnaryRpc::new(abortable, self.message_id, self.outbound.clone())
    }

    /// Consume the responder by providing a stream that will materialize the response to this request.
    pub fn stream(
        self,
        streaming_rpc: impl futures::Stream<Item = Response>,
    ) -> impl Future<Output = ()> {
        let (abortable_stream, abort) = IdentifiableAbortable::new(streaming_rpc);
        self.aborts.register(self.message_id, abort);
        ForwardAbortableStreamingRpc::new(abortable_stream, self.message_id, self.outbound.clone())
    }

    /// Consume the responder by providing an immediate response.
    ///
    /// This is the cheapest, fastest way to respond, but you must only use it when you can get a response
    /// without blocking!
    pub fn immediate(self, response: Response) {
        if self
            .outbound
            .send(RpcResponse::Untracked(response))
            .is_err()
        {
            log::debug!("outbound channel closed while sending response");
        }
    }
}

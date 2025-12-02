use std::{future::Future, pin::pin};

use crate::{
    server::{
        abortable::IdentifiableAbortable, abortion_tracker::AbortionTracker,
        forward::ForwardAbortableUnaryRpc,
    },
    Message,
};

#[must_use]
pub struct RpcResponder<'a, Response> {
    outbound: &'a spillway::Sender<Response>,
    aborts: &'a std::sync::Arc<AbortionTracker>,
    message_id: u64,
}
impl<'a, Response> RpcResponder<'a, Response>
where
    Response: Message,
{
    pub(crate) fn new_responder_reference(
        outbound: &'a spillway::Sender<Response>,
        aborts: &'a std::sync::Arc<AbortionTracker>,
        message_id: u64,
    ) -> Self {
        Self {
            outbound,
            aborts,
            message_id,
        }
    }

    pub fn unary(self, unary_rpc: impl Future<Output = Response>) -> impl Future<Output = ()> {
        let (abortable, abort) = IdentifiableAbortable::new(unary_rpc);
        self.aborts.register(self.message_id, abort);
        ForwardAbortableUnaryRpc::new(
            abortable,
            self.message_id,
            self.outbound.clone(),
            self.aborts.clone(),
        )
    }

    pub fn stream(
        self,
        streaming_rpc: impl futures::Stream<Item = Response>,
    ) -> impl Future<Output = ()> {
        let outbound = self.outbound.clone();
        async move {
            let mut streaming_rpc = pin!(streaming_rpc);
            while let Some(next) = futures::StreamExt::next(&mut streaming_rpc).await {
                if outbound.send(next).is_err() {
                    log::debug!("outbound channel closed during streaming response");
                    break;
                }
            }
        }
    }

    pub fn immediate(self, response: Response) {
        if self.outbound.send(response).is_err() {
            log::debug!("outbound channel closed while sending response");
        }
    }
}

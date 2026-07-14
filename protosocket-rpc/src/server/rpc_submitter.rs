use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};

use futures::stream::SelectAll;
use futures::Stream;
use protosocket::MessageReactor;

use crate::{
    server::{abortion_tracker::AbortionTracker, ConnectionService, RpcKind},
    Message, ProtosocketControlCode,
};

use super::rpc_stream::{RpcStream, RpcStreamEvent};

/// A MessageReactor that hosts a ConnectionService's rpcs and drives them within the
/// connection's send budget.
///
/// New rpcs are registered from inbound messages. Their completions - unary and streaming
/// alike - live in one pool and are only advanced by `poll_next_outbound`, which the
/// connection calls only when it has room to send. This is the backpressure contract: a
/// connection that cannot write does not advance the work that produces responses. The
/// pool yields in readiness order; no priority between unary and streaming rpcs is imposed.
pub struct RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    connection_server: TConnectionService,
    aborts: AbortionTracker,
    /// Message ids of rpcs to reject. Small and self-limiting: bounded by the inbound
    /// messages processed between sends.
    rejections: VecDeque<u64>,
    rpcs: SelectAll<RpcStream<TConnectionService::UnaryFutureType, TConnectionService::StreamType>>,
}

impl<TConnectionService> std::fmt::Debug for RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcSubmitter")
            .field("aborts", &self.aborts)
            .field("rejections", &self.rejections.len())
            .field("rpcs", &self.rpcs.len())
            .finish()
    }
}

impl<TConnectionService> RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    pub fn new(connection_server: TConnectionService) -> Self {
        Self {
            connection_server,
            aborts: Default::default(),
            rejections: Default::default(),
            rpcs: Default::default(),
        }
    }
}

impl<TConnectionService> MessageReactor for RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    type Inbound = TConnectionService::Request;
    type Outbound = TConnectionService::Response;
    type LogicalOutbound = TConnectionService::Response;

    fn on_inbound_message(&mut self, message: Self::Inbound) -> protosocket::ReactorStatus {
        let message_id = message.message_id();
        match message.control_code() {
            ProtosocketControlCode::Normal => match self.connection_server.new_rpc(message) {
                RpcKind::Unary(completion) => {
                    let (rpc, handle) = RpcStream::new_unary(message_id, completion);
                    self.aborts.register(message_id, handle);
                    self.rpcs.push(rpc);
                }
                RpcKind::Streaming(stream) => {
                    let (rpc, handle) = RpcStream::new_streaming(message_id, stream);
                    self.aborts.register(message_id, handle);
                    self.rpcs.push(rpc);
                }
                RpcKind::Cancelled => {
                    log::debug!("rejecting unknown rpc {message_id}");
                    self.rejections.push_back(message_id);
                }
            },
            ProtosocketControlCode::Cancel => {
                if let Some(abort) = self.aborts.take_abort(message_id) {
                    log::debug!("cancelling message {message_id}");
                    abort.mark_aborted();
                } else {
                    log::debug!("received cancellation for untracked message {message_id}");
                }
            }
            ProtosocketControlCode::End => {
                log::debug!("received end message {message_id}");
            }
        }
        protosocket::ReactorStatus::Continue
    }

    fn on_outbound_message(&mut self, response: Self::LogicalOutbound) -> Self::Outbound {
        response
    }

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> std::ops::ControlFlow<()> {
        // SAFETY: This is a structural pin. If I'm not moved then neither is this service.
        let structurally_pinned_connection_server = unsafe {
            self.as_mut()
                .map_unchecked_mut(|me| &mut me.connection_server)
        };
        structurally_pinned_connection_server.poll(context)
    }

    /// Produce the next response, in rpc readiness order. This is only called when the
    /// connection can accept a message for serialization, so rpc work only advances when
    /// its output has somewhere to go.
    fn poll_next_outbound(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Self::LogicalOutbound>> {
        let me = self.get_mut();
        if let Some(message_id) = me.rejections.pop_front() {
            return Poll::Ready(Some(<Self::Outbound as Message>::cancelled(message_id)));
        }
        match Pin::new(&mut me.rpcs).poll_next(context) {
            Poll::Ready(Some((id, event))) => match event {
                RpcStreamEvent::Item(mut item) => {
                    item.set_message_id(id);
                    Poll::Ready(Some(item))
                }
                RpcStreamEvent::Complete(mut response) => {
                    let _ = me.aborts.take_abort(id);
                    response.set_message_id(id);
                    Poll::Ready(Some(response))
                }
                RpcStreamEvent::Finished => {
                    let _ = me.aborts.take_abort(id);
                    Poll::Ready(Some(<Self::Outbound as Message>::ended(id)))
                }
                RpcStreamEvent::Cancelled => {
                    let _ = me.aborts.take_abort(id);
                    Poll::Ready(Some(<Self::Outbound as Message>::cancelled(id)))
                }
            },
            // An empty pool is not a closed reactor: new rpcs arrive from inbound
            // processing, which wakes this connection on its own.
            Poll::Ready(None) | Poll::Pending => Poll::Pending,
        }
    }
}

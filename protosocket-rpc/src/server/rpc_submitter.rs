use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};

use futures::stream::{FuturesUnordered, StreamFuture};
use futures::{Stream, StreamExt};
use protosocket::MessageReactor;

use crate::{
    Message, ProtosocketControlCode,
    server::{ConnectionService, RpcKind, abortion_tracker::AbortionTracker},
};

use super::rpc_stream::{RpcStream, RpcStreamEvent};

/// How many messages to take from one rpc before returning it to the pool. Re-inserting
/// a stream into the pool costs a node cycle per visit, so hot streams are drained in
/// small batches to amortize it. This bounds both the staging queue and how far one rpc
/// can burst ahead of its peers within a send budget.
const PER_RPC_BATCH: usize = 64;

type PooledRpc<TConnectionService> = RpcStream<
    <TConnectionService as ConnectionService>::UnaryFutureType,
    <TConnectionService as ConnectionService>::StreamType,
>;

/// A MessageReactor that hosts a ConnectionService's rpcs and drives them within the
/// connection's send budget.
///
/// New rpcs are registered from inbound messages. Their completions - unary and streaming
/// alike - live in one pool and are only advanced by `poll_next_outbound`, which the
/// connection calls only when it has room to send. This is the backpressure contract: a
/// connection that cannot write does not advance the work that produces responses. The
/// pool yields in readiness order; no priority between unary and streaming rpcs is
/// imposed, and a ready rpc yields at most `PER_RPC_BATCH` messages per pool visit.
pub struct RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    connection_server: TConnectionService,
    aborts: AbortionTracker,
    /// Message ids of rpcs to reject. Small and self-limiting: bounded by the inbound
    /// messages processed between sends.
    rejections: VecDeque<u64>,
    rpcs: FuturesUnordered<StreamFuture<PooledRpc<TConnectionService>>>,
    /// Messages already claimed from a batched rpc, awaiting hand-off to the connection.
    /// Bounded by `PER_RPC_BATCH`.
    staged: VecDeque<TConnectionService::Response>,
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
            .field("staged", &self.staged.len())
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
            staged: Default::default(),
        }
    }

    /// Turn an rpc event into the message that goes on the wire, retiring the rpc's
    /// cancellation bookkeeping on terminal events.
    fn message_for_event(
        &mut self,
        id: u64,
        event: RpcStreamEvent<TConnectionService::Response>,
    ) -> TConnectionService::Response {
        match event {
            RpcStreamEvent::Item(mut item) => {
                item.set_message_id(id);
                item
            }
            RpcStreamEvent::Complete(mut response) => {
                let _ = self.aborts.take_abort(id);
                response.set_message_id(id);
                response
            }
            RpcStreamEvent::Finished => {
                let _ = self.aborts.take_abort(id);
                <TConnectionService::Response as Message>::ended(id)
            }
            RpcStreamEvent::Cancelled => {
                let _ = self.aborts.take_abort(id);
                <TConnectionService::Response as Message>::cancelled(id)
            }
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
                    self.rpcs.push(rpc.into_future());
                }
                RpcKind::Streaming(stream) => {
                    let (rpc, handle) = RpcStream::new_streaming(message_id, stream);
                    self.aborts.register(message_id, handle);
                    self.rpcs.push(rpc.into_future());
                }
                RpcKind::Cancelled => {
                    log::debug!("rejecting rpc {message_id}");
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
        if let Some(staged) = me.staged.pop_front() {
            return Poll::Ready(Some(staged));
        }
        if let Some(message_id) = me.rejections.pop_front() {
            return Poll::Ready(Some(<Self::Outbound as Message>::cancelled(message_id)));
        }
        loop {
            match Pin::new(&mut me.rpcs).poll_next(context) {
                Poll::Ready(Some((first, mut rpc))) => {
                    let Some((id, event)) = first else {
                        // The rpc's terminal event was already delivered; retire it and
                        // look at the next ready rpc.
                        continue;
                    };
                    let terminal = !matches!(event, RpcStreamEvent::Item(_));
                    let first = me.message_for_event(id, event);
                    if terminal {
                        return Poll::Ready(Some(first));
                    }
                    // Drain a small batch from this rpc while it is hot, so a busy stream
                    // doesn't pay a pool re-insertion per message.
                    let mut exhausted = false;
                    while me.staged.len() < PER_RPC_BATCH - 1 {
                        match Pin::new(&mut rpc).poll_next(context) {
                            Poll::Ready(Some((id, event))) => {
                                let terminal = !matches!(event, RpcStreamEvent::Item(_));
                                let message = me.message_for_event(id, event);
                                me.staged.push_back(message);
                                if terminal {
                                    exhausted = true;
                                    break;
                                }
                            }
                            Poll::Ready(None) => {
                                exhausted = true;
                                break;
                            }
                            Poll::Pending => break,
                        }
                    }
                    if !exhausted {
                        me.rpcs.push(rpc.into_future());
                    }
                    return Poll::Ready(Some(first));
                }
                // An empty pool is not a closed reactor: new rpcs arrive from inbound
                // processing, which wakes this connection on its own.
                Poll::Ready(None) | Poll::Pending => return Poll::Pending,
            }
        }
    }
}

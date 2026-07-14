use std::{
    pin::{Pin, pin},
    task::{Context, Poll},
};

use futures::stream::{FuturesUnordered, StreamFuture};
use futures::{Stream, StreamExt};
use protosocket::{MessageReactor, SendManyPermit, SendPermit};

use crate::{
    Message, ProtosocketControlCode,
    server::{ConnectionService, RpcKind, abortion_tracker::AbortionTracker},
};

use super::rpc_stream::{RpcStream, RpcStreamEvent};

type PooledRpc<TConnectionService> = RpcStream<
    <TConnectionService as ConnectionService>::UnaryFutureType,
    <TConnectionService as ConnectionService>::StreamType,
>;

/// A MessageReactor that hosts a ConnectionService's rpcs.
///
/// New rpcs are registered from inbound messages. Their completions live in a pool and
/// are advanced by `poll_outbound_many`. The connection only polls outbound when it
/// has room to send. If you spawn your rpc's, you'll probably also want to limit concurrent
/// rpcs, or you'll only get one-sided backpressure.
/// A connection that cannot write does not advance the response futures.
///
/// Rpc messages are sent in readiness order. There is no ordering across rpcs, however
/// streaming rpcs still get relative ordering for their own messages. You'll receive streams
/// in the order they were yielded by your rpc.
pub struct RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    connection_server: TConnectionService,
    aborts: AbortionTracker,
    /// Message ids of rpcs to reject. Small and self-limiting: bounded by the inbound
    /// messages processed between sends.
    rejections: Vec<u64>,
    rpcs: FuturesUnordered<StreamFuture<PooledRpc<TConnectionService>>>,
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
                    self.rejections.push(message_id);
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

    /// Produce responses in rpc readiness order. Ready rpcs are drained in runs to
    /// amortize pool re-insertion; an rpc that exhausts the budget goes to the back.
    fn poll_outbound_many(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        outbound: &mut impl protosocket::SendBudget<Self::Outbound>,
    ) -> Poll<Option<()>> {
        let me = self.get_mut();
        let mut produced = 0usize;
        if !me.rejections.is_empty() {
            let mut permits = outbound.reserve_many(me.rejections.len());
            for message_id in me.rejections.drain(..permits.reserved()) {
                permits.send(<Self::Outbound as Message>::cancelled(message_id));
                produced += 1;
            }
        }
        loop {
            let Some(permit) = outbound.reserve() else {
                break;
            };
            match pin!(&mut me.rpcs).poll_next(context) {
                Poll::Ready(Some((first, mut rpc))) => {
                    let Some((id, event)) = first else {
                        // StreamFuture yielded None: this rpc's stream was already
                        // exhausted, and there's no more work to do for it.
                        continue;
                    };
                    let terminal = !matches!(event, RpcStreamEvent::Item(_));
                    permit.send(me.message_for_event(id, event));
                    produced += 1;
                    if terminal {
                        continue;
                    }
                    let retired = loop {
                        let Some(permit) = outbound.reserve() else {
                            break false;
                        };
                        match pin!(&mut rpc).poll_next(context) {
                            Poll::Ready(Some((id, event))) => {
                                let terminal = !matches!(event, RpcStreamEvent::Item(_));
                                permit.send(me.message_for_event(id, event));
                                produced += 1;
                                if terminal {
                                    break true;
                                }
                            }
                            Poll::Ready(None) => break true,
                            Poll::Pending => break false,
                        }
                    };
                    if !retired {
                        me.rpcs.push(rpc.into_future());
                    }
                }
                // An empty pool is not a finished reactor: new rpcs arrive from inbound
                // processing, which wakes this connection on its own.
                Poll::Ready(None) | Poll::Pending => break,
            }
        }
        if 0 < produced {
            Poll::Ready(Some(()))
        } else {
            Poll::Pending
        }
    }
}

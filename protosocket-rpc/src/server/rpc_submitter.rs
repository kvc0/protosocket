use std::{
    collections::HashMap,
    pin::{pin, Pin},
    task::{Context, Poll},
};

use futures::stream::{FuturesUnordered, SelectAll};
use protosocket::{MessageReactor, ReactorStatus};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::{
    server::{
        abortable::{AbortableState, IdentifiableAbortHandle, IdentifiableAbortable},
        ConnectionService, RpcKind,
    },
    Message, ProtosocketControlCode,
};

/// A MessageReactor that sends RPCs along to a sink
#[derive(Debug)]
pub struct RpcSubmitter<TConnectionServer>
where
    TConnectionServer: ConnectionService,
{
    connection_server: TConnectionServer,
    outbound: PollSender<<TConnectionServer as ConnectionService>::Response>,
    aborts: HashMap<u64, IdentifiableAbortHandle>,
    outstanding_unary_rpcs:
        FuturesUnordered<IdentifiableAbortable<TConnectionServer::UnaryFutureType>>,
    outstanding_streaming_rpcs: SelectAll<IdentifiableAbortable<TConnectionServer::StreamType>>,
}
impl<TConnectionService> RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    pub fn new(
        connection_server: TConnectionService,
        outbound: mpsc::Sender<TConnectionService::Response>,
    ) -> Self {
        Self {
            connection_server,
            outbound: PollSender::new(outbound),
            aborts: Default::default(),
            outstanding_unary_rpcs: Default::default(),
            outstanding_streaming_rpcs: Default::default(),
        }
    }
}

impl<TConnectionService> MessageReactor for RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    type Inbound = TConnectionService::Request;

    fn on_inbound_message(&mut self, message: Self::Inbound) -> protosocket::ReactorStatus {
        let message_id = message.message_id();
        match message.control_code() {
            ProtosocketControlCode::Normal => match self.connection_server.new_rpc(message) {
                RpcKind::Unary(completion) => {
                    let (abortable, abort) = IdentifiableAbortable::new(message_id, completion);
                    self.aborts.insert(message_id, abort);
                    self.outstanding_unary_rpcs.push(abortable);
                }
                RpcKind::Streaming(completion) => {
                    let (completion, abort) = IdentifiableAbortable::new(message_id, completion);
                    self.aborts.insert(message_id, abort);
                    self.outstanding_streaming_rpcs.push(completion);
                }
                RpcKind::Unknown => {
                    log::debug!("skipping message {message_id}");
                }
            },
            ProtosocketControlCode::Cancel => {
                if let Some(abort) = self.aborts.remove(&message_id) {
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

    fn poll(
        mut self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<protosocket::ReactorStatus> {
        // retire and advance outstanding rpcs
        if let Some(_early_out) = self.as_mut().poll_advance_unary_rpcs(context) {
            return Poll::Ready(ReactorStatus::Disconnect);
        }
        if let Some(_early_out) = self.poll_advance_streaming_rpcs(context) {
            return Poll::Ready(ReactorStatus::Disconnect);
        }

        std::task::Poll::Ready(protosocket::ReactorStatus::Continue)
    }
}

impl<TConnectionService> RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    fn poll_advance_unary_rpcs(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), crate::Error>>> {
        if self.outstanding_unary_rpcs.is_empty() {
            log::trace!("no outstanding unary rpcs to advance");
            return None;
        }

        loop {
            match pin!(&mut self.outbound).poll_reserve(context) {
                Poll::Ready(Ok(())) => {
                    log::trace!("outbound connection is ready");
                    // ready to send
                }
                Poll::Ready(Err(_)) => {
                    log::debug!("outbound connection is closed");
                    return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                }
                Poll::Pending => {
                    log::debug!("no room in outbound connection");
                    break;
                }
            }

            match futures::Stream::poll_next(pin!(&mut self.outstanding_unary_rpcs), context) {
                Poll::Ready(unary_done) => {
                    match unary_done {
                        Some((id, AbortableState::Ready(Ok(response)))) => {
                            self.aborts.remove(&id);
                            log::trace!("{id} unary rpc response {response:?}");
                            if let Err(_e) = self.outbound.send_item(response) {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                            }
                        }
                        Some((id, AbortableState::Ready(Err(e)))) => {
                            let abort = self.aborts.remove(&id);
                            match e {
                                crate::Error::IoFailure(error) => {
                                    log::warn!("{id} io failure while servicing rpc: {error:?}");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                crate::Error::CancelledRemotely => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                crate::Error::ConnectionIsClosed => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                crate::Error::Finished => {
                                    log::debug!("{id} unary rpc ended");
                                    if let Some(abort) = abort {
                                        if let Err(_e) = self.outbound.send_item(
                                            <TConnectionService::Response as Message>::ended(id),
                                        ) {
                                            log::debug!("outbound connection is closed");
                                            return Some(Poll::Ready(Err(
                                                crate::Error::ConnectionIsClosed,
                                            )));
                                        }

                                        abort.mark_aborted();
                                    }
                                }
                            }
                            // cancelled
                        }
                        Some((id, AbortableState::Abort)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} unary rpc abort");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.abort();
                            }
                        }
                        Some((id, AbortableState::Aborted)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} unary rpc done");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.mark_aborted();
                            }
                        }
                        None => {
                            log::trace!(
                                "no outstanding unary rpcs, not registered for wake on unary rpcs"
                            );
                            break;
                        }
                    }
                }
                Poll::Pending => {
                    log::trace!(
                        "pending on unary rpcs: {}",
                        self.outstanding_unary_rpcs.len()
                    );
                    break;
                }
            }
        }
        None
    }

    // I want to join this with the above function but it is annoying to zip the SelectAll and FuturesUnordered together.
    // This should be possible today with futures::Stream but I need to sit and stare at it for a while to figure out how.
    fn poll_advance_streaming_rpcs(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), crate::Error>>> {
        if self.outstanding_streaming_rpcs.is_empty() {
            log::trace!("no outstanding streaming rpcs to advance");
            return None;
        }
        loop {
            match pin!(&mut self.outbound).poll_reserve(context) {
                Poll::Ready(Ok(())) => {
                    // ready to send
                }
                Poll::Ready(Err(_)) => {
                    log::debug!("outbound connection is closed");
                    return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                }
                Poll::Pending => {
                    log::debug!("no room in outbound connection");
                    break;
                }
            }

            match futures::Stream::poll_next(pin!(&mut self.outstanding_streaming_rpcs), context) {
                Poll::Ready(streaming_next) => {
                    match streaming_next {
                        Some((id, AbortableState::Ready(Ok(next)))) => {
                            log::debug!("{id} streaming rpc next {next:?}");
                            if let Err(_e) = self.outbound.send_item(next) {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                            }
                        }
                        Some((id, AbortableState::Ready(Err(e)))) => {
                            let abort = self.aborts.remove(&id);
                            match e {
                                crate::Error::IoFailure(error) => {
                                    log::warn!("{id} io failure while servicing rpc: {error:?}");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                crate::Error::CancelledRemotely => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                crate::Error::ConnectionIsClosed => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                crate::Error::Finished => {
                                    log::debug!("{id} streaming rpc ended");
                                    if let Some(abort) = abort {
                                        if let Err(_e) = self.outbound.send_item(
                                            <TConnectionService::Response as Message>::ended(id),
                                        ) {
                                            log::debug!("outbound connection is closed");
                                            return Some(Poll::Ready(Err(
                                                crate::Error::ConnectionIsClosed,
                                            )));
                                        }
                                        abort.mark_aborted();
                                    }
                                }
                            }
                        }
                        Some((id, AbortableState::Abort)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} streaming rpc abort");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.abort();
                            }
                        }
                        Some((id, AbortableState::Aborted)) => {
                            log::debug!("{id} streaming rpc done");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.mark_aborted();
                            }
                        }
                        None => {
                            // nothing to wait for
                            break;
                        }
                    }
                }
                Poll::Pending => break,
            }
        }
        None
    }
}

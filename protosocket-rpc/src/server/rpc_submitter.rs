use protosocket::MessageReactor;

use crate::{
    server::{abortion_tracker::AbortionTracker, rpc_responder::RpcResponder, ConnectionService},
    Message, ProtosocketControlCode,
};

/// A MessageReactor that sends RPCs along to a sink
#[derive(Debug)]
pub struct RpcSubmitter<TConnectionServer>
where
    TConnectionServer: ConnectionService,
{
    connection_server: TConnectionServer,
    outbound: spillway::Sender<<TConnectionServer as ConnectionService>::Response>,
    aborts: AbortionTracker,
}
impl<TConnectionService> RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    pub fn new(
        connection_server: TConnectionService,
        outbound: spillway::Sender<TConnectionService::Response>,
    ) -> Self {
        Self {
            connection_server,
            outbound,
            aborts: Default::default(),
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
            ProtosocketControlCode::Normal => {
                self.connection_server.new_rpc(
                    message,
                    RpcResponder::new_responder_reference(
                        &self.outbound,
                        &mut self.aborts,
                        message_id,
                    ),
                );
            }
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

    fn on_outbound_message(&mut self, message: Self::LogicalOutbound) -> Self::Outbound {
        message
    }
}

impl<TConnectionService> RpcSubmitter<TConnectionService>
where
    TConnectionService: ConnectionService,
{
    // fn poll_advance_streaming_rpcs(
    //     mut self: Pin<&mut Self>,
    //     context: &mut Context<'_>,
    // ) -> Option<Poll<Result<(), crate::Error>>> {
    //     if self.outstanding_streaming_rpcs.is_empty() {
    //         log::trace!("no outstanding streaming rpcs to advance");
    //         return None;
    //     }
    //     while let Poll::Ready(streaming_next) =
    //         futures::Stream::poll_next(pin!(&mut self.outstanding_streaming_rpcs), context)
    //     {
    //         match streaming_next {
    //             Some((id, AbortableState::Ready(Ok(next)))) => {
    //                 log::debug!("{id} streaming rpc next {next:?}");
    //                 if let Err(_e) = self.outbound.send(next) {
    //                     log::debug!("outbound connection is closed");
    //                     return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
    //                 }
    //             }
    //             Some((id, AbortableState::Ready(Err(e)))) => {
    //                 let abort = self.aborts.remove(&id);
    //                 match e {
    //                     crate::Error::IoFailure(error) => {
    //                         log::warn!("{id} io failure while servicing rpc: {error:?}");
    //                         if let Some(abort) = abort {
    //                             abort.abort();
    //                         }
    //                     }
    //                     crate::Error::CancelledRemotely => {
    //                         log::debug!("{id} rpc cancelled remotely");
    //                         if let Some(abort) = abort {
    //                             abort.abort();
    //                         }
    //                     }
    //                     crate::Error::ConnectionIsClosed => {
    //                         log::debug!("{id} rpc cancelled remotely");
    //                         if let Some(abort) = abort {
    //                             abort.abort();
    //                         }
    //                     }
    //                     crate::Error::Finished => {
    //                         log::debug!("{id} streaming rpc ended");
    //                         if let Some(abort) = abort {
    //                             if let Err(_e) = self
    //                                 .outbound
    //                                 .send(<TConnectionService::Response as Message>::ended(id))
    //                             {
    //                                 log::debug!("outbound connection is closed");
    //                                 return Some(Poll::Ready(Err(
    //                                     crate::Error::ConnectionIsClosed,
    //                                 )));
    //                             }
    //                             abort.mark_aborted();
    //                         }
    //                     }
    //                 }
    //             }
    //             Some((id, AbortableState::Abort)) => {
    //                 // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
    //                 log::debug!("{id} streaming rpc abort");
    //                 if let Some(abort) = self.aborts.remove(&id) {
    //                     abort.abort();
    //                 }
    //             }
    //             Some((id, AbortableState::Aborted)) => {
    //                 log::debug!("{id} streaming rpc done");
    //                 if let Some(abort) = self.aborts.remove(&id) {
    //                     abort.mark_aborted();
    //                 }
    //             }
    //             None => {
    //                 // nothing to wait for
    //                 break;
    //             }
    //         }
    //     }
    //     None
    // }
}

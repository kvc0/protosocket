use std::{
    future::Future, pin::{Pin, pin}, sync::Arc, task::{Context, Poll}
};

use protosocket::{MessageReactor, ReactorStatus};

use crate::{
    Message, ProtosocketControlCode, server::{
        ConnectionService, RpcKind, abortable::{AbortableState, IdentifiableAbortable}, abortion_tracker::AbortionTracker
    }
};

/// A MessageReactor that sends RPCs along to a sink
#[derive(Debug)]
pub struct RpcSubmitter<TConnectionServer>
where
    TConnectionServer: ConnectionService,
{
    connection_server: TConnectionServer,
    outbound: spillway::Sender<<TConnectionServer as ConnectionService>::Response>,
    aborts: Arc<AbortionTracker>,
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

    fn on_inbound_message(&mut self, message: Self::Inbound) -> protosocket::ReactorStatus {
        let message_id = message.message_id();
        match message.control_code() {
            ProtosocketControlCode::Normal => match self.connection_server.new_rpc(message) {
                RpcKind::Unary(rpc_task) => {
                    let (abortable, abort) = IdentifiableAbortable::new(rpc_task);
                    self.aborts.register(message_id, abort);
                    let unary_rpc_task = ForwardAbortableUnaryRpc::new(abortable, message_id, self.outbound.clone(), self.aborts.clone());
                    // todo: spawn
                    // self.outstanding_unary_rpcs.push(abortable);
                }
                RpcKind::Streaming(rpc_task) => {
                    let (completion, abort) = IdentifiableAbortable::new(rpc_task);
                    self.aborts.register(message_id, abort);
                    self.outstanding_streaming_rpcs.push(completion);
                }
                RpcKind::Unknown => {
                    log::debug!("skipping message {message_id}");
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

struct ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    future: F,
    id: u64,
    forward: spillway::Sender<T>,
    aborts: Arc<AbortionTracker>,
}
impl<F, T> Drop for ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    fn drop(&mut self) {
        self.aborts.take_abort(self.id);
    }
}
impl<F, T> ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    pub fn new(future: F, id: u64, forward: spillway::Sender<T>, aborts: Arc<AbortionTracker>) -> Self {
        Self {
            future,
            id,
            forward,
            aborts,
        }
    }
}
impl<F, T> Future for ForwardAbortableUnaryRpc<F, T> where F: Future<Output = AbortableState<crate::Result<T>>>, T: Message {
    type Output = ();
    
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: This is a structural pin. If I'm not moved then neither is this future.
        let structurally_pinned_future =
            unsafe { self.as_mut().map_unchecked_mut(|me| &mut me.future) };
        match structurally_pinned_future.poll(context) {
            Poll::Ready(state) => {
                let abort = self.aborts.take_abort(self.id);
                match state {
                    AbortableState::Ready(Ok(response)) => {
                        log::trace!("{} unary rpc response", self.id);
                        if let Err(_e) = self.forward.send(response) {
                            log::debug!("outbound connection is closed");
                        }
                        Poll::Ready(())
                    }
                    AbortableState::Ready(Err(e)) => {
                        match e {
                            crate::Error::IoFailure(error) => {
                                log::warn!("{} io failure while servicing rpc: {error:?}", self.id);
                                if let Some(abort) = abort {
                                    abort.abort();
                                }
                            }
                            crate::Error::CancelledRemotely => {
                                log::debug!("{} rpc cancelled remotely", self.id);
                                if let Some(abort) = abort {
                                    abort.abort();
                                }
                            }
                            crate::Error::ConnectionIsClosed => {
                                log::debug!("{} rpc cancelled remotely", self.id);
                                if let Some(abort) = abort {
                                    abort.abort();
                                }
                            }
                            crate::Error::Finished => {
                                log::debug!("{} unary rpc ended", self.id);
                                if let Some(abort) = abort {
                                    if let Err(_e) = self.forward.send(
                                        T::ended(self.id),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                    }
                                    abort.mark_aborted();
                                }
                            }
                        }
                        Poll::Ready(())
                        // cancelled
                    }
                    AbortableState::Abort => {
                        // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                        log::debug!("{} unary rpc abort", self.id);
                        if let Some(abort) = abort {
                            abort.abort();
                        }
                        Poll::Ready(())
                    }
                    AbortableState::Aborted => {
                        // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                        log::debug!("{} unary rpc done", self.id);
                        if let Some(abort) = abort {
                            abort.mark_aborted();
                        }
                        Poll::Ready(())
                    }
                }
            }
            Poll::Pending => {
                Poll::Pending
            }
        }
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
            match futures::Stream::poll_next(pin!(&mut self.outstanding_unary_rpcs), context) {
                Poll::Ready(unary_done) => {
                    match unary_done {
                        Some((id, AbortableState::Ready(Ok(response)))) => {
                            self.aborts.remove(&id);
                            log::trace!("{id} unary rpc response {response:?}");
                            if let Err(_e) = self.outbound.send(response) {
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
                                        if let Err(_e) = self.outbound.send(
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
        while let Poll::Ready(streaming_next) =
            futures::Stream::poll_next(pin!(&mut self.outstanding_streaming_rpcs), context)
        {
            match streaming_next {
                Some((id, AbortableState::Ready(Ok(next)))) => {
                    log::debug!("{id} streaming rpc next {next:?}");
                    if let Err(_e) = self.outbound.send(next) {
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
                                if let Err(_e) = self
                                    .outbound
                                    .send(<TConnectionService::Response as Message>::ended(id))
                                {
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
        None
    }
}

use std::{
    collections::HashMap,
    future::Future,
    pin::{pin, Pin},
    task::{Context, Poll},
};

use futures::{
    stream::{FuturesUnordered, SelectAll},
    Stream,
};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::{server::RpcKind, Error, Message, ProtosocketControlCode};

use super::{
    abortable::{IdentifiableAbortHandle, IdentifiableAbortable},
    ConnectionServer,
};

pub struct RpcConnectionServer<TConnectionServer>
where
    TConnectionServer: ConnectionServer,
{
    connection_server: TConnectionServer,
    inbound: mpsc::UnboundedReceiver<<TConnectionServer as ConnectionServer>::Request>,
    outbound: PollSender<<TConnectionServer as ConnectionServer>::Response>,
    next_messages_buffer: Vec<<TConnectionServer as ConnectionServer>::Request>,
    outstanding_unary_rpcs:
        FuturesUnordered<IdentifiableAbortable<TConnectionServer::UnaryCompletion>>,
    outstanding_streaming_rpcs:
        SelectAll<IdentifiableAbortable<TConnectionServer::StreamingCompletion>>,
    aborts: HashMap<u64, IdentifiableAbortHandle>,
}

impl<TConnectionServer> Future for RpcConnectionServer<TConnectionServer>
where
    TConnectionServer: ConnectionServer,
{
    type Output = Result<(), crate::Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // retire and advance outstanding rpcs
        if let Some(early_out) = self.as_mut().poll_advance_unary_rpcs(context) {
            return early_out;
        }
        if let Some(early_out) = self.as_mut().poll_advance_streaming_rpcs(context) {
            return early_out;
        }

        // receive new messages
        if let Some(early_out) = self.as_mut().poll_receive_buffer(context) {
            return early_out;
        }
        // either we're pending on inbound or we're awake

        self.handle_message_buffer();

        Poll::Pending
    }
}

impl<TConnectionServer> RpcConnectionServer<TConnectionServer>
where
    TConnectionServer: ConnectionServer,
{
    pub fn new(
        connection_server: TConnectionServer,
        inbound: mpsc::UnboundedReceiver<<TConnectionServer as ConnectionServer>::Request>,
        outbound: mpsc::Sender<<TConnectionServer as ConnectionServer>::Response>,
    ) -> Self {
        Self {
            connection_server,
            inbound,
            outbound: PollSender::new(outbound),
            next_messages_buffer: Default::default(),
            outstanding_unary_rpcs: Default::default(),
            outstanding_streaming_rpcs: Default::default(),
            aborts: Default::default(),
        }
    }

    fn poll_advance_unary_rpcs(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), Error>>> {
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

            match pin!(&mut self.outstanding_unary_rpcs).poll_next(context) {
                Poll::Ready(unary_done) => {
                    match unary_done {
                        Some((id, Some(Ok(response)))) => {
                            self.aborts.remove(&id);
                            if let Err(_e) = self.outbound.send_item(response) {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                            }
                        }
                        Some((id, Some(Err(e)))) => {
                            match e {
                                Error::IoFailure(error) => {
                                    log::warn!("{id} io failure while servicing rpc: {error:?}");
                                    if let Err(_e) = self.outbound.send_item(
                                        <TConnectionServer::Response as Message>::cancelled(),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                                Error::CancelledRemotely => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Err(_e) = self.outbound.send_item(
                                        <TConnectionServer::Response as Message>::cancelled(),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                                Error::ConnectionIsClosed => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Err(_e) = self.outbound.send_item(
                                        <TConnectionServer::Response as Message>::cancelled(),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                                Error::Finished => {
                                    log::debug!("{id} rpc ended");
                                    if let Err(_e) =
                                        self.outbound.send_item(
                                            <TConnectionServer::Response as Message>::ended(),
                                        )
                                    {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                            }
                            // cancelled
                        }
                        Some((id, None)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} rpc dropped");
                            if let Err(_e) = self
                                .outbound
                                .send_item(<TConnectionServer::Response as Message>::cancelled())
                            {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
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

    // I want to join this with the above function but it is annoying to zip the SelectAll and FuturesUnordered together.
    // This should be possible today with futures::Stream but I need to sit and stare at it for a while to figure out how.
    fn poll_advance_streaming_rpcs(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), Error>>> {
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

            match pin!(&mut self.outstanding_streaming_rpcs).poll_next(context) {
                Poll::Ready(unary_done) => {
                    match unary_done {
                        Some((id, Some(Ok(response)))) => {
                            self.aborts.remove(&id);
                            if let Err(_e) = self.outbound.send_item(response) {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                            }
                        }
                        Some((id, Some(Err(e)))) => {
                            match e {
                                Error::IoFailure(error) => {
                                    log::warn!("{id} io failure while servicing rpc: {error:?}");
                                    if let Err(_e) = self.outbound.send_item(
                                        <TConnectionServer::Response as Message>::cancelled(),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                                Error::CancelledRemotely => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Err(_e) = self.outbound.send_item(
                                        <TConnectionServer::Response as Message>::cancelled(),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                                Error::ConnectionIsClosed => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Err(_e) = self.outbound.send_item(
                                        <TConnectionServer::Response as Message>::cancelled(),
                                    ) {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                                Error::Finished => {
                                    log::debug!("{id} rpc ended");
                                    if let Err(_e) =
                                        self.outbound.send_item(
                                            <TConnectionServer::Response as Message>::ended(),
                                        )
                                    {
                                        log::debug!("outbound connection is closed");
                                        return Some(Poll::Ready(Err(
                                            crate::Error::ConnectionIsClosed,
                                        )));
                                    }
                                }
                            }
                        }
                        Some((id, None)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} rpc dropped");
                            if let Err(_e) = self
                                .outbound
                                .send_item(<TConnectionServer::Response as Message>::cancelled())
                            {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
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

    fn poll_receive_buffer(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), Error>>> {
        if self.next_messages_buffer.is_empty() {
            let Self {
                inbound,
                next_messages_buffer,
                ..
            } = &mut *self;
            match inbound.poll_recv_many(context, next_messages_buffer, 64) {
                Poll::Ready(0) => {
                    return Some(Poll::Ready(Ok(())));
                }
                Poll::Ready(_count) => {
                    // possible there is more, but let's just do one batch at a time
                    context.waker().wake_by_ref();
                }
                Poll::Pending => {}
            }
        }
        None
    }

    fn handle_message_buffer(mut self: Pin<&mut Self>) {
        while let Some(next_message) = self.next_messages_buffer.pop() {
            let message_id = next_message.message_id();
            match next_message.control_code() {
                ProtosocketControlCode::Normal => {
                    match self.connection_server.new_rpc(next_message) {
                        RpcKind::Unary(completion) => {
                            let (completion, abort) =
                                IdentifiableAbortable::new(message_id, completion);
                            self.aborts.insert(message_id, abort);
                            self.outstanding_unary_rpcs.push(completion);
                        }
                        RpcKind::Streaming(completion) => {
                            let (completion, abort) =
                                IdentifiableAbortable::new(message_id, completion);
                            self.aborts.insert(message_id, abort);
                            self.outstanding_streaming_rpcs.push(completion);
                        }
                    }
                }
                ProtosocketControlCode::Cancel => {
                    if let Some(abort) = self.aborts.remove(&message_id) {
                        log::debug!("cancelling message {message_id}");
                        abort.abort();
                    } else {
                        log::debug!("received cancellation for untracked message {message_id}");
                    }
                }
                ProtosocketControlCode::End => {
                    if let Some(abort) = self.aborts.remove(&message_id) {
                        log::debug!("ending message {message_id}");
                        abort.abort();
                    } else {
                        log::debug!("received end for untracked message {message_id}");
                    }
                }
            }
        }
    }
}

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{Connection, ConnectionBindings, Deserializer, NetworkStatusEvent};
use tokio::sync::mpsc;

#[derive(Debug, PartialEq, Eq)]
pub enum ReactorStatus {
    Continue,
    Disconnect,
}

pub trait MessageReactor: Unpin {
    type Inbound;

    /// you must take all of the messages quickly. If you need to disconnect, return err.
    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus;
}

/// To be spawned, this is the task that handles serialization, deserialization, and network readiness events.
pub struct ConnectionDriver<
    Bindings: ConnectionBindings,
    Reactor: MessageReactor<Inbound = <Bindings::Deserializer as Deserializer>::Message>,
> {
    connection: Connection<Bindings>,
    network_readiness: mpsc::UnboundedReceiver<NetworkStatusEvent>,
    message_reactor: Reactor,
}

impl<
        Bindings: ConnectionBindings,
        Reactor: MessageReactor<Inbound = <Bindings::Deserializer as Deserializer>::Message>,
    > ConnectionDriver<Bindings, Reactor>
{
    pub fn new(
        connection: Connection<Bindings>,
        network_readiness: mpsc::UnboundedReceiver<NetworkStatusEvent>,
        message_reactor: Reactor,
    ) -> Self {
        Self {
            connection,
            network_readiness,
            message_reactor,
        }
    }
}

impl<
        Bindings: ConnectionBindings,
        Reactor: MessageReactor<Inbound = <Bindings::Deserializer as Deserializer>::Message>,
    > ConnectionDriver<Bindings, Reactor>
{
    fn poll_network_status(&mut self, context: &mut Context<'_>) -> Poll<()> {
        loop {
            break match self.network_readiness.poll_recv(context) {
                Poll::Ready(Some(event)) => {
                    let connection_is_closed = self.connection.handle_connection_event(event);
                    if connection_is_closed {
                        return Poll::Ready(());
                    }
                    continue;
                }
                Poll::Ready(None) => {
                    log::debug!("dropping connection: network readiness sender dropped");
                    Poll::Ready(())
                }
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

/// safety: no unsafe code in here
impl<
        Bindings: ConnectionBindings,
        Reactor: MessageReactor<Inbound = <Bindings::Deserializer as Deserializer>::Message>,
    > Unpin for ConnectionDriver<Bindings, Reactor>
{
}

impl<
        Bindings: ConnectionBindings,
        Reactor: MessageReactor<Inbound = <Bindings::Deserializer as Deserializer>::Message>,
    > Future for ConnectionDriver<Bindings, Reactor>
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Poll::Ready(early_out) = self.poll_network_status(context) {
            return Poll::Ready(early_out);
        }

        if let Poll::Ready(early_out) = self.connection.poll_write_outbound_messages(context) {
            log::debug!("dropping connection: outbound channel failure");
            return Poll::Ready(early_out);
        }

        match self.connection.poll_write_buffers(context) {
            Ok(closed) => {
                if closed {
                    log::info!("write connection closed");
                    return Poll::Ready(());
                }
            }
            Err(e) => {
                log::warn!("dropping connection: write failed {e:?}");
                return Poll::Ready(());
            }
        }

        match self.connection.poll_read_inbound(context) {
            Ok(false) => {
                // log::trace!("checked inbound connection buffer");
            }
            Ok(true) => {
                if self.connection.has_work_in_flight() {
                    log::debug!("read closed connection but work is in flight");
                    return Poll::Ready(());
                } else {
                    log::debug!("read closed connection");
                    return Poll::Ready(());
                }
            }
            Err(e) => {
                log::warn!("dropping connection after read: {e:?}");
                return Poll::Ready(());
            }
        }

        match self
            .connection
            .poll_read_inbound_messages_into_read_queue(context)
        {
            Ok(false) => {
                // log::trace!("checked for messages to dispatch");
            }
            Ok(true) => {
                if self.connection.has_work_in_flight() {
                    log::debug!("connection read dispatch is closed but work is in flight");
                    return Poll::Ready(());
                } else {
                    log::debug!("connection read dispatch is closed");
                    return Poll::Ready(());
                }
            }
            Err(e) => {
                log::warn!("dropping connection after read dispatch: {e:?}");
                return Poll::Ready(());
            }
        }

        let Self {
            connection,
            message_reactor,
            ..
        } = &mut *self;
        if message_reactor.on_inbound_messages(connection.drain_inbound_messages())
            == ReactorStatus::Disconnect
        {
            log::debug!("reactor requested disconnect");
            return Poll::Ready(());
        }

        Poll::Pending
    }
}

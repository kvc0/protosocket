use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use protosocket::{Connection, ConnectionBindings, NetworkStatusEvent};
use tokio::sync::mpsc;

/// To be spawned, this is the task that handles serialization, deserialization, and network readiness events.
pub struct ConnectionDriver<Bindings: ConnectionBindings> {
    connection: Connection<Bindings>,
    network_readiness: mpsc::UnboundedReceiver<NetworkStatusEvent>,
}

impl<Bindings: ConnectionBindings> ConnectionDriver<Bindings> {
    pub(crate) fn new(
        connection: Connection<Bindings>,
        network_readiness: mpsc::UnboundedReceiver<NetworkStatusEvent>,
    ) -> Self {
        Self {
            connection,
            network_readiness,
        }
    }
}

impl<Bindings: ConnectionBindings> ConnectionDriver<Bindings> {
    fn poll_network_status(&mut self, context: &mut Context<'_>) -> Poll<()> {
        loop {
            break match self.network_readiness.poll_recv(context) {
                Poll::Ready(Some(event)) => {
                    self.connection.handle_connection_event(event);
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

impl<Bindings: ConnectionBindings> Future for ConnectionDriver<Bindings> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Poll::Ready(early_out) = self.poll_network_status(context) {
            return Poll::Ready(early_out);
        }

        if let Poll::Ready(early_out) = self.connection.poll_serialize_oubound(context) {
            log::debug!("dropping connection: outbound channel failure");
            return Poll::Ready(early_out);
        }

        if let Err(e) = self.connection.poll_write_buffers() {
            log::warn!("dropping connection: write failed {e:?}");
            return Poll::Ready(());
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

        Poll::Pending
    }
}

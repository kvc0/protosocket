use std::{
    cmp::{max, min},
    collections::HashMap,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use mio::{net::TcpStream, Interest, Token};
use protosocket::{Connection, ConnectionBindings, NetworkStatusEvent};
use tokio::sync::{mpsc, oneshot};

use crate::{Error, ProstClientConnectionBindings, ProstSerializer};

pub struct ClientRegistry {
    new_clients: mpsc::UnboundedSender<RegisterClient>,
}

impl ClientRegistry {
    pub fn new() -> crate::Result<(Self, ClientRegistryDriver)> {
        log::trace!("new client registry");
        let (sender, receiver) = mpsc::unbounded_channel();

        Ok((
            Self {
                new_clients: sender,
            },
            ClientRegistryDriver::new(receiver)?,
        ))
    }

    pub async fn register_client<Request, Response>(
        &self,
        address: impl Into<String>,
    ) -> crate::Result<(
        mpsc::Sender<Request>,
        mpsc::Receiver<Response>,
        ConnectionDriver<ProstClientConnectionBindings<Request, Response>>,
    )>
    where
        Request: prost::Message + Default + Unpin,
        Response: prost::Message + Default + Unpin,
    {
        let address = address.into().parse()?;
        let stream = TcpStream::connect(address)?;

        let (completion, registration) = oneshot::channel();
        self.new_clients
            .send(RegisterClient { stream, completion })
            .map_err(|_e| Error::Dead("client registry driver is dead"))?;
        let RegisteredClient {
            stream,
            network_readiness,
        } = registration.await.map_err(|_e| Error::Dead("canceled"))?;

        let (outbound, inbound, connection) =
            Connection::<ProstClientConnectionBindings<Request, Response>>::new(
                stream,
                ProstSerializer::default(),
                ProstSerializer::default(),
            );
        let connection_driver = ConnectionDriver {
            connection,
            network_readiness,
        };
        Ok((outbound, inbound, connection_driver))
    }
}

struct RegisterClient {
    stream: TcpStream,
    completion: oneshot::Sender<RegisteredClient>,
}

struct RegisteredClient {
    stream: TcpStream,
    network_readiness: mpsc::UnboundedReceiver<NetworkStatusEvent>,
}

pub struct ClientRegistryDriver {
    new_clients: mpsc::UnboundedReceiver<RegisterClient>,
    poll: mio::Poll,
    events: mio::Events,
    clients: HashMap<Token, mpsc::UnboundedSender<NetworkStatusEvent>>,
    client_counter: usize,
    /// used to let the server settle into waiting for readiness
    poll_backoff: Duration,
}

impl ClientRegistryDriver {
    fn new(new_clients: mpsc::UnboundedReceiver<RegisterClient>) -> crate::Result<Self> {
        let poll = mio::Poll::new()?;
        let events = mio::Events::with_capacity(1024);
        Ok(Self {
            new_clients,
            poll,
            events,
            clients: Default::default(),
            client_counter: 0,
            poll_backoff: Duration::from_millis(1),
        })
    }

    fn poll_new_connections(&mut self, context: &mut Context<'_>) -> Poll<()> {
        loop {
            break match self.new_clients.poll_recv(context) {
                Poll::Ready(Some(mut registration)) => {
                    let token = Token(self.client_counter);
                    self.client_counter += 1;
                    match self.poll.registry().register(
                        &mut registration.stream,
                        token,
                        Interest::READABLE | Interest::WRITABLE,
                    ) {
                        Ok(_) => {
                            let (readiness_sender, network_readiness) = mpsc::unbounded_channel();
                            self.clients.insert(token, readiness_sender);

                            let _ = registration.completion.send(RegisteredClient {
                                stream: registration.stream,
                                network_readiness,
                            });
                            continue;
                        }
                        Err(e) => {
                            log::error!("failed to register stream: {e:?}");
                            Poll::Ready(())
                        }
                    }
                }
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => {
                    log::debug!("registry was dropped");
                    Poll::Ready(())
                }
            };
        }
    }

    fn poll_mio(
        &mut self,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<<Self as std::future::Future>::Output> {
        // FIXME: schedule this task to wake up again in a smarter way. This just makes sure events aren't missed.....
        context.waker().wake_by_ref();
        if let Err(e) = self.poll.poll(&mut self.events, Some(self.poll_backoff)) {
            log::error!("failed to poll connections: {e:?}");
            return std::task::Poll::Ready(());
        }

        if self.events.is_empty() {
            self.decrease_poll_rate()
        } else {
            self.increase_poll_rate()
        }

        for event in self.events.iter() {
            let token = event.token();
            let event: NetworkStatusEvent = match event.try_into() {
                Ok(e) => e,
                Err(_) => continue,
            };
            if let Some(readiness) = self.clients.get_mut(&token) {
                if let Err(_e) = readiness.send(event) {
                    log::debug!("client dropped");
                    return Poll::Ready(());
                }
            } else {
                log::debug!(
                    "something happened for a socket that isn't connected anymore {event:?}"
                );
            }
        }
        std::task::Poll::Pending
    }

    fn increase_poll_rate(&mut self) {
        self.poll_backoff = max(Duration::from_micros(1), self.poll_backoff / 2);
    }

    fn decrease_poll_rate(&mut self) {
        self.poll_backoff = min(
            Duration::from_millis(100),
            self.poll_backoff + Duration::from_micros(10),
        );
    }
}

impl Future for ClientRegistryDriver {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if let Poll::Ready(early_out) = self.poll_new_connections(context) {
            return Poll::Ready(early_out);
        }
        if let Poll::Ready(early_out) = self.poll_mio(context) {
            return Poll::Ready(early_out);
        }
        Poll::Pending
    }
}

pub struct ConnectionDriver<Bindings: ConnectionBindings> {
    connection: Connection<Bindings>,
    network_readiness: mpsc::UnboundedReceiver<NetworkStatusEvent>,
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

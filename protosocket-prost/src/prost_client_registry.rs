use std::{
    cmp::{max, min},
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::atomic::AtomicUsize,
    task::{Context, Poll},
    thread::JoinHandle,
    time::Duration,
};

use mio::{net::TcpStream, Interest, Token};
use protosocket::{Connection, ConnectionDriver, MessageReactor, NetworkStatusEvent};
use tokio::sync::{mpsc, oneshot};

use crate::{Error, ProstClientConnectionBindings, ProstSerializer};

/// The root IO tracker for a class of clients.
/// You can use different kinds of clients with a single registry. The registry generates network status
/// events for connections - readable & writable. Connections themselves service the events and invoke
/// read/writev.
#[derive(Debug, Clone)]
pub struct ClientRegistry {
    new_clients: mpsc::UnboundedSender<RegisterClient>,
    max_message_length: usize,
    max_queued_outbound_messages: usize,
}

impl ClientRegistry {
    /// Construct a new client registry. You will spawn the registry driver, probably on a dedicated thread.
    pub fn new() -> crate::Result<(Self, ClientRegistryDriver)> {
        log::trace!("new client registry");
        let (sender, receiver) = mpsc::unbounded_channel();

        Ok((
            Self {
                new_clients: sender,
                max_message_length: 4 * (2 << 20),
                max_queued_outbound_messages: 256,
            },
            ClientRegistryDriver::new(receiver)?,
        ))
    }

    pub fn set_max_message_length(&mut self, max_message_length: usize) {
        self.max_message_length = max_message_length;
    }

    pub fn set_max_queued_outbound_messages(&mut self, max_queued_outbound_messages: usize) {
        self.max_queued_outbound_messages = max_queued_outbound_messages;
    }

    /// Get a new connection to a protosocket server.
    pub async fn register_client<Request, Response, Reactor>(
        &self,
        address: impl Into<String>,
        message_reactor: Reactor,
    ) -> crate::Result<(
        mpsc::Sender<Request>,
        ConnectionDriver<ProstClientConnectionBindings<Request, Response>, Reactor>,
    )>
    where
        Request: prost::Message + Default + Unpin,
        Response: prost::Message + Default + Unpin,
        Reactor: MessageReactor<Inbound = Response>,
    {
        let address = address.into().parse()?;
        let stream = TcpStream::connect(address).map_err(std::sync::Arc::new)?;

        let (completion, registration) = oneshot::channel();
        self.new_clients
            .send(RegisterClient { stream, completion })
            .map_err(|_e| Error::Dead("client registry driver is dead"))?;
        let RegisteredClient {
            stream,
            network_readiness,
        } = registration.await.map_err(|_e| Error::Dead("canceled"))?;

        let (outbound, connection) =
            Connection::<ProstClientConnectionBindings<Request, Response>>::new(
                stream,
                ProstSerializer::default(),
                ProstSerializer::default(),
                self.max_message_length,
                self.max_queued_outbound_messages,
            );
        let connection_driver =
            ConnectionDriver::new(connection, network_readiness, message_reactor);
        Ok((outbound, connection_driver))
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

/// You may choose to spawn this in your mixed runtime, but you should strongly consider putting
/// it on a dedicated thread. It uses epoll and though it tries to stay live, it can hitch for brief
/// moments.
pub struct ClientRegistryDriver {
    new_clients: mpsc::UnboundedReceiver<RegisterClient>,
    poll: mio::Poll,
    events: mio::Events,
    clients: HashMap<Token, mpsc::UnboundedSender<NetworkStatusEvent>>,
    client_counter: usize,
    /// used to let the server settle into waiting for readiness
    poll_backoff: Duration,
    max_poll_backoff: Duration,
}

impl ClientRegistryDriver {
    fn new(new_clients: mpsc::UnboundedReceiver<RegisterClient>) -> crate::Result<Self> {
        let poll = mio::Poll::new().map_err(std::sync::Arc::new)?;
        let events = mio::Events::with_capacity(1024);
        Ok(Self {
            new_clients,
            poll,
            events,
            clients: Default::default(),
            client_counter: 0,
            poll_backoff: Duration::from_millis(1),
            max_poll_backoff: Duration::from_millis(100),
        })
    }

    pub fn set_max_poll_backoff(&mut self, max_poll_backoff: Duration) {
        // mio uses 1 millisecond as the minumum, but it might do something different in the future.
        self.max_poll_backoff = max(max_poll_backoff, Duration::from_micros(1));
    }

    /// launch this client registry's IO on a background thread protoskt-{i}.
    ///
    /// Consider this as your default choice for a client registry.
    pub fn handle_io_on_dedicated_thread(self) -> crate::Result<JoinHandle<()>> {
        static I: AtomicUsize = AtomicUsize::new(0);
        let io = std::thread::Builder::new()
            .name(format!(
                "protoskt-{}",
                I.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("io thread can have a runtime");
                runtime.block_on(self);
            })
            .map_err(std::sync::Arc::new)?;

        Ok(io)
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
            if event == NetworkStatusEvent::Closed {
                // final readiness event
                if let Some(readiness) = self.clients.remove(&token) {
                    let _ = readiness.send(event);
                }
            } else if let Some(readiness) = self.clients.get_mut(&token) {
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
            self.max_poll_backoff,
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

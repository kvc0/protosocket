use std::cmp::max;
use std::cmp::min;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::pin;
use std::task::Poll;
use std::time::Duration;

use mio::Interest;
use mio::Token;
use protosocket::Connection;
use protosocket::ConnectionBindings;
use protosocket::ConnectionDriver;
use protosocket::Deserializer;
use protosocket::MessageReactor;
use protosocket::NetworkStatusEvent;
use protosocket::Serializer;
use tokio::sync::mpsc;

use crate::connection_acceptor::NewConnection;

pub trait ServerConnector: Unpin {
    type Bindings: ConnectionBindings;
    type Reactor: MessageReactor<
        Inbound = <<Self::Bindings as ConnectionBindings>::Deserializer as Deserializer>::Message,
    >;

    fn serializer(&self) -> <Self::Bindings as ConnectionBindings>::Serializer;
    fn deserializer(&self) -> <Self::Bindings as ConnectionBindings>::Deserializer;

    fn new_reactor(
        &self,
        optional_outbound: mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
    ) -> Self::Reactor;

    fn take_new_connection(
        &self,
        address: SocketAddr,
        outbound: mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
        connection_driver: ConnectionDriver<Self::Bindings, Self::Reactor>,
    );

    fn maximum_message_length(&self) -> usize {
        4 * (2 << 20)
    }

    fn max_queued_outbound_messages(&self) -> usize {
        256
    }
}

/// A ConnectionServer is an IO driver. It directly uses mio to poll the OS's io primitives,
/// manages read and write buffers, and vends messages to & from connections.
/// Connections interact with the ConnectionServer through mpsc channels.
///
/// Protosockets are monomorphic messages - you can only have 1 kind of message per service.
/// The expected way to work with this is to use prost and protocol buffers to encode messages.
///
/// Protosocket messages are not opinionated about request & reply. If you are, you will need
/// to implement such a thing. This allows you freely choose whether you want to send
/// fire-&-forget messages sometimes; however it requires you to write your protocol's rules.
/// You get an inbound stream of <MessageIn> and an outbound stream of <MessageOut> per
/// connection - you decide what those streams mean for you!
pub struct ConnectionServer<Connector: ServerConnector> {
    new_streams: mpsc::UnboundedReceiver<NewConnection>,
    connection_token_count: usize,
    connections_network_status: HashMap<Token, mpsc::UnboundedSender<NetworkStatusEvent>>,
    poll: mio::Poll,
    events: mio::Events,
    server_state: Connector,
    /// used to let the server settle into waiting for readiness
    poll_backoff: Duration,
    max_poll_backoff: Duration,
}

impl<Connector: ServerConnector> std::future::Future for ConnectionServer<Connector> {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        if let std::task::Poll::Ready(early_out) =
            self.as_mut().poll_register_new_connections(context)
        {
            return std::task::Poll::Ready(early_out);
        }

        if let std::task::Poll::Ready(early_out) = self.poll_mio(context) {
            return std::task::Poll::Ready(early_out);
        }

        std::task::Poll::Pending
    }
}

impl<Connector: ServerConnector> ConnectionServer<Connector> {
    pub(crate) fn new(
        server_state: Connector,
        new_streams: mpsc::UnboundedReceiver<NewConnection>,
    ) -> crate::Result<Self> {
        Ok(Self {
            new_streams,
            connection_token_count: 0,
            connections_network_status: Default::default(),
            poll: mio::Poll::new().map_err(std::sync::Arc::new)?,
            events: mio::Events::with_capacity(1024),
            server_state,
            poll_backoff: Duration::from_millis(100),
            max_poll_backoff: Duration::from_millis(100),
        })
    }

    pub fn set_max_poll_backoff(&mut self, max_poll_backoff: Duration) {
        // mio uses 1 millisecond as the minimum, but it might do something different in the future.
        self.max_poll_backoff = max(max_poll_backoff, Duration::from_micros(1));
    }

    fn poll_register_new_connections(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<<Self as std::future::Future>::Output> {
        loop {
            break match pin!(&mut self.new_streams).poll_recv(context) {
                std::task::Poll::Ready(Some(mut new_connection)) => {
                    let token = Token(self.connection_token_count);
                    self.connection_token_count += 1;

                    if let Err(e) = self.poll.registry().register(
                        &mut new_connection.stream,
                        token,
                        Interest::READABLE.add(Interest::WRITABLE),
                    ) {
                        log::error!("failed to register stream: {e:?}");
                        std::task::Poll::Ready(())
                    } else {
                        let serializer = self.server_state.serializer();
                        let deserializer = self.server_state.deserializer();
                        let (outbound, connection) = Connection::<Connector::Bindings>::new(
                            new_connection.stream,
                            deserializer,
                            serializer,
                            self.server_state.maximum_message_length(),
                            self.server_state.max_queued_outbound_messages(),
                        );

                        let (readiness_sender, network_readiness) = mpsc::unbounded_channel();
                        let connection_driver = ConnectionDriver::new(
                            connection,
                            network_readiness,
                            self.server_state.new_reactor(outbound.clone()),
                        );
                        self.connections_network_status
                            .insert(token, readiness_sender);

                        self.server_state.take_new_connection(
                            new_connection.address,
                            outbound,
                            connection_driver,
                        );

                        continue;
                    }
                }
                std::task::Poll::Ready(None) => {
                    log::warn!("listener closed");
                    return std::task::Poll::Ready(());
                }
                std::task::Poll::Pending => std::task::Poll::Pending,
            };
        }
    }

    fn increase_poll_rate(&mut self) {
        self.poll_backoff = max(Duration::from_micros(1), self.poll_backoff / 4);
    }

    fn decrease_poll_rate(&mut self) {
        self.poll_backoff = min(
            self.max_poll_backoff,
            self.poll_backoff + Duration::from_micros(10),
        );
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
                Err(_) => {
                    log::debug!("received unknown event");
                    continue;
                }
            };
            if let Some(connection) = self.connections_network_status.get_mut(&token) {
                if let Err(e) = connection.send(event) {
                    log::debug!("client dropped: {e:?}");
                }
            } else {
                log::debug!(
                    "something happened for a socket that isn't connected anymore {event:?}"
                );
            }
        }
        Poll::Pending
    }
}

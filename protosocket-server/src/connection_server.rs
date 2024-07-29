use std::cmp::max;
use std::cmp::min;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::pin;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use mio::net::TcpListener;
use mio::Interest;
use mio::Token;
use protosocket::Connection;
use protosocket::ConnectionBindings;
use protosocket::Deserializer;
use protosocket::NetworkStatusEvent;
use protosocket::Serializer;
use tokio::sync::mpsc;

use crate::connection_acceptor::ConnectionAcceptor;
use crate::connection_acceptor::NewConnection;

pub trait ServerConnector: Unpin {
    type Bindings: ConnectionBindings;

    fn serializer(&self) -> <Self::Bindings as ConnectionBindings>::Serializer;
    fn deserializer(&self) -> <Self::Bindings as ConnectionBindings>::Deserializer;
    fn take_new_connection(
        &self,
        address: SocketAddr,
        outbound: mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
        inbound: mpsc::Receiver<
            <<Self::Bindings as ConnectionBindings>::Deserializer as Deserializer>::Message,
        >,
    );
}

/// Once you've configured your connection server the way you want it, execute it on your asynchronous runtime.
pub struct ConnectionServer<Connector: ServerConnector> {
    new_streams: mpsc::UnboundedReceiver<NewConnection>,
    connection_token_count: usize,
    connections: HashMap<Token, Connection<Connector::Bindings>>,
    poll: mio::Poll,
    events: mio::Events,
    server_state: Connector,
    /// used to let the server settle into waiting for readiness
    poll_backoff: Duration,
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

        self.poll_connections(context);

        std::task::Poll::Pending
    }
}

impl<Connector: ServerConnector> ConnectionServer<Connector> {
    pub(crate) fn new(
        server_state: Connector,
        listener: TcpListener,
    ) -> crate::Result<(ConnectionAcceptor, Self)> {
        let (inbound_streams, new_streams) = mpsc::unbounded_channel();

        Ok((
            ConnectionAcceptor::new(listener, inbound_streams),
            Self {
                new_streams,
                connection_token_count: 0,
                connections: Default::default(),
                poll: mio::Poll::new()?,
                events: mio::Events::with_capacity(1024),
                server_state,
                poll_backoff: Duration::from_millis(200),
            },
        ))
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
                        let (outbound, inbound, connection) =
                            Connection::new(new_connection.stream, deserializer, serializer);
                        self.server_state.take_new_connection(
                            new_connection.address,
                            outbound,
                            inbound,
                        );

                        self.connections.insert(token, connection);
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
            Duration::from_millis(100),
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
                Err(_) => continue,
            };
            if let Some(connection) = self.connections.get_mut(&token) {
                connection.handle_connection_event(event);
            } else {
                log::debug!(
                    "something happened for a socket that isn't connected anymore {event:?}"
                );
            }
        }
        Poll::Pending
    }

    fn poll_connections(mut self: Pin<&mut Self>, context: &mut Context) {
        let mut drop_connections = Vec::new();
        for (token, connection) in self.connections.iter_mut() {
            match connection.poll_read_inbound(context) {
                Ok(false) => {
                    // log::trace!("checked inbound connection buffer");
                }
                Ok(true) => {
                    if connection.has_work_in_flight() {
                        log::debug!("connection read is closed but work is in flight");
                        drop_connections.push(*token);
                    } else {
                        log::debug!("connection read is closed");
                        drop_connections.push(*token);
                    }
                }
                Err(e) => {
                    log::warn!("dropping connection after read: {e:?}");
                    drop_connections.push(*token);
                }
            }

            if let Poll::Ready(_early_out) = connection.poll_serialize_oubound(context) {
                log::warn!("dropping connection for outbound serialization failure");
                drop_connections.push(*token);
                continue;
            }

            if let Err(e) = connection.poll_write_buffers() {
                log::warn!("dropping connection for write buffer failure: {e:?}");
                drop_connections.push(*token);
                continue;
            }
        }
        for connection in drop_connections {
            if let Some(connection) = self.connections.remove(&connection) {
                connection.deregister(self.poll.registry());
            }
        }
    }
}

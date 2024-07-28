use std::cmp::max;
use std::cmp::min;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::future::Future;
use std::io::IoSlice;
use std::io::Read;
use std::io::Write;
use std::pin::pin;
use std::time::Duration;

use futures::channel::mpsc;
use futures::Stream;
use mio::net::TcpListener;
use mio::Interest;
use mio::Token;

use crate::connection::Connection;
use crate::connection_acceptor::ConnectionAcceptor;
use crate::connection_acceptor::NewConnection;
use crate::interrupted;
use crate::types::ConnectionLifecycle;
use crate::types::DeserializeError;
use crate::would_block;
use crate::Error;
use crate::Result;

/// Once you've configured your connection server the way you want it, execute it on your asynchronous runtime.
pub struct ConnectionServer<Lifecycle: ConnectionLifecycle> {
    new_streams: mpsc::UnboundedReceiver<NewConnection>,
    connection_token_count: usize,
    connections: HashMap<Token, Connection<Lifecycle>>,
    mio_io: Option<Mio>,
    server_state: Lifecycle::ServerState,
    /// used to let the server settle into waiting for readiness
    poll_backoff: Duration,
}

struct Mio {
    poll: mio::Poll,
    events: mio::Events,
}

impl<Lifecycle: ConnectionLifecycle> std::future::Future for ConnectionServer<Lifecycle> {
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

        let Mio {
            mut poll,
            mut events,
        } = self.mio_io.take().expect("state must have mio");
        if let std::task::Poll::Ready(early_out) = self.poll_mio(context, &mut poll, &mut events) {
            return std::task::Poll::Ready(early_out);
        }
        self.mio_io = Mio { poll, events }.into();

        self.poll_connections(context);

        std::task::Poll::Pending
    }
}

impl<Lifecycle: ConnectionLifecycle> ConnectionServer<Lifecycle> {
    pub(crate) fn new(
        server_state: Lifecycle::ServerState,
        listener: TcpListener,
    ) -> (ConnectionAcceptor, Self) {
        let (inbound_streams, new_streams) = mpsc::unbounded();

        (
            ConnectionAcceptor::new(
                listener,
                inbound_streams,
            ),
            Self {
                new_streams,
                connection_token_count: 0,
                connections: Default::default(),
                mio_io: Mio {
                    poll: mio::Poll::new().expect("must be able to create a poll"),
                    events: mio::Events::with_capacity(1024),
                }
                .into(),
                server_state,
                poll_backoff: Duration::from_millis(200),
            },
        )
    }

    fn poll_register_new_connections(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<<Self as std::future::Future>::Output> {
        loop {
            break match pin!(&mut self.new_streams).poll_next(context) {
                std::task::Poll::Ready(Some(mut connection)) => {
                    let token = Token(self.connection_token_count);
                    self.connection_token_count += 1;

                    if let Err(e) = self
                        .mio_io
                        .as_mut()
                        .expect("must have mio")
                        .poll
                        .registry()
                        .register(
                            &mut connection.stream,
                            token,
                            Interest::READABLE.add(Interest::WRITABLE),
                        )
                    {
                        log::error!("failed to register stream: {e:?}");
                        std::task::Poll::Ready(())
                    } else {
                        let (lifecycle, deserializer, serializer) =
                            Lifecycle::on_connect(&self.server_state);
                        self.connections.insert(
                            token,
                            Connection::new(connection, lifecycle, deserializer, serializer),
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
        self.poll_backoff = max(Duration::from_micros(1), self.poll_backoff / 2);
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
        poll: &mut mio::Poll,
        events: &mut mio::Events,
    ) -> std::task::Poll<<Self as std::future::Future>::Output> {
        // FIXME: schedule this task to wake up again in a smarter way. This just makes sure events aren't missed.....
        context.waker().wake_by_ref();
        if let Err(e) = poll.poll(events, Some(self.poll_backoff)) {
            log::error!("failed to poll connections: {e:?}");
            return std::task::Poll::Ready(());
        }

        if events.is_empty() {
            self.decrease_poll_rate()
        } else {
            self.increase_poll_rate()
        }

        for event in events.iter() {
            let token = event.token();
            if let Some(connection) = self.connections.get_mut(&token) {
                connection.handle_mio_connection_event(event);
            } else {
                log::debug!(
                    "something happened for a socket that isn't connected anymore {event:?}"
                );
            }
        }
        std::task::Poll::Pending
    }

    fn poll_connections(mut self: std::pin::Pin<&mut Self>, context: &mut std::task::Context) {
        let mut drop_connections = Vec::new();
        for (token, connection) in self.connections.iter_mut() {
            match connection.poll_read_buffer() {
                Ok(false) => {
                    // log::trace!("checked inbound connection buffer");
                }
                Ok(true) => {
                    if connection.has_work_in_flight() {
                        log::trace!("remote closed connection but work is in flight - extending lifetime a little");
                    } else {
                        log::trace!("remote closed connection");
                        drop_connections.push(*token);
                    }
                }
                Err(e) => {
                    log::warn!("dropping connection after read: {e:?}");
                    drop_connections.push(*token);
                }
            }

            connection.poll_serialize_completions(context);

            if let Err(e) = connection.poll_write_buffers() {
                log::warn!("dropping connection: {e:?}");
                drop_connections.push(*token);
                continue;
            }
        }
        for connection in drop_connections {
            if let Some(mut connection) = self.connections.remove(&connection) {
                connection.deregister(
                    self.mio_io
                    .as_mut()
                    .expect("mio must exist")
                    .poll
                    .registry());
            }
        }
    }
}

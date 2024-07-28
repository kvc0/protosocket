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

use crate::interrupted;
use crate::Error;
use crate::Result;

#[derive(Debug)]
pub(crate) struct NewConnection {
    stream: mio::net::TcpStream,
    address: std::net::SocketAddr,
}

#[derive(Debug)]
pub(crate) struct Connection<Lifecycle: ConnectionLifecycle> {
    stream: mio::net::TcpStream,
    address: std::net::SocketAddr,
    lifecycle: Lifecycle,
    serializer_buffers: Vec<Vec<u8>>,
    send_buffer: VecDeque<Vec<u8>>,
    receive_buffer_length: usize,
    receive_buffer: Vec<u8>,
    deserializer: Lifecycle::Deserializer,
    serializer: Lifecycle::Serializer,
    work_in_progress: futures::stream::FuturesUnordered<Lifecycle::MessageFuture>,
    readable: bool,
    writable: bool,
}

impl<Lifecycle: ConnectionLifecycle> Drop for Connection<Lifecycle> {
    fn drop(&mut self) {
        log::debug!("connection dropped")
    }
}

impl<Lifecycle: ConnectionLifecycle> Connection<Lifecycle> {
    fn handle_mio_connection_event(&mut self, event: &mio::event::Event) {
        if event.is_writable() {
            log::trace!("connection is writable");
            self.writable = true;
        }

        if event.is_readable() {
            log::trace!("connection is readable");
            self.readable = true;
        }
    }

    fn read_buffer(&mut self) -> std::result::Result<bool, std::io::Error> {
        if !self.readable {
            return Ok(false);
        }

        const BUFFER_INCREMENT: usize = 16 * (2 << 10);
        if self.receive_buffer.len() - self.receive_buffer_length < BUFFER_INCREMENT {
            self.receive_buffer.resize(
                min(
                    2 << 20,
                    max(BUFFER_INCREMENT, self.receive_buffer.len() / 4),
                ),
                0,
            );
        }

        // We can (maybe) read from the connection.
        match self
            .stream
            .read(&mut self.receive_buffer[self.receive_buffer_length..])
        {
            Ok(0) => {
                // Reading 0 bytes means the other side has closed the
                // connection.
                return Ok(true);
            }
            Ok(bytes_read) => {
                self.receive_buffer_length += bytes_read;

                let mut consumed = 0;

                while consumed < self.receive_buffer_length {
                    let buffer = &self.receive_buffer[consumed..self.receive_buffer_length];
                    match self.deserializer.decode(buffer) {
                        Ok((length, message)) => {
                            consumed += length;
                            self.work_in_progress
                                .push(self.lifecycle.on_message(message));
                        }
                        Err(e) => {
                            match e {
                                DeserializeError::IncompleteBuffer { next_message_size } => {
                                    log::debug!("waiting for the next message of length {next_message_size}");
                                }
                                DeserializeError::InvalidMessage => {
                                    log::debug!("message was invalid");
                                    consumed = self.receive_buffer_length;
                                }
                            }
                        }
                    }
                }
                self.receive_buffer_length -= consumed;
                if consumed != 0 && self.receive_buffer_length != 0 {
                    // partial read - have to shift the buffer because I haven't made it smarter.
                    // at least I'm pulling multiple messages out if there's a bunch of little messages in a big buffer...
                    self.receive_buffer.rotate_left(consumed);
                }
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {
                log::trace!("read everything. No longer readable");
                self.readable = false;
            }
            Err(ref err) if interrupted(err) => {
                log::trace!("interrupted, so try again later");
            }
            // Other errors we'll consider fatal.
            Err(err) => return Err(err),
        }
        Ok(false)
    }

    fn write_buffers(&mut self) -> std::result::Result<(), std::io::Error> {
        if !self.writable {
            return Ok(());
        }
        let buffers: Vec<IoSlice> = self.send_buffer.iter().map(|v| IoSlice::new(&v)).collect();
        match self.stream.write_vectored(&buffers) {
            Ok(written) => {
                self.rotate_send_buffers(written);
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {
                log::trace!("would block - no longer writable");
                self.writable = false;
            }
            Err(ref err) if interrupted(err) => {
                log::trace!("write interrupted - try again later");
            }
            // other errors terminate the stream
            Err(err) => return Err(err),
        }
        Ok(())
    }

    fn rotate_send_buffers(&mut self, mut written: usize) {
        while let Some(mut front) = self.send_buffer.pop_front() {
            if front.len() < written {
                written -= front.len();

                // Reuse the buffer!
                // SAFETY: This is purely a buffer, and u8 does not require drop.
                unsafe { front.set_len(0) };
                self.serializer_buffers.push(front);
            } else {
                // Walk the buffer forward through a replacement. It will still amortize the allocation,
                // but this is not optimal. It's relatively easier to manage though, and I'm a busy person.
                let replacement = front[written..].to_vec();
                self.send_buffer.push_front(replacement);
            }
        }
    }
}

pub trait Deserializer: Unpin {
    type Request;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Request), DeserializeError>;
}

pub trait Serializer: Unpin {
    type Response;

    fn encode(&mut self, response: Self::Response, buffer: &mut impl bytes::BufMut);
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    /// Buffer will be retained and you will be called again later with more bytes
    #[error("Need more bytes to decode the next message")]
    IncompleteBuffer { next_message_size: usize },
    /// Buffer will be discarded
    #[error("Bad message")]
    InvalidMessage,
}

pub trait ConnectionLifecycle: Unpin + Sized {
    type ServerState: Unpin;
    type Deserializer: Deserializer;
    type Serializer: Serializer;
    type MessageFuture: Future<Output = <Self::Serializer as Serializer>::Response>;

    /// A new connection lifecycle starts here. If you have a state machine, initialize it here. This is your constructor.
    fn on_connect(server_state: &Self::ServerState)
        -> (Self, Self::Deserializer, Self::Serializer);

    fn on_message(
        &self,
        message: <Self::Deserializer as Deserializer>::Request,
    ) -> Self::MessageFuture;
}

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

impl<Lifecycle: ConnectionLifecycle> ConnectionServer<Lifecycle> {
    pub(crate) fn new(
        server_state: Lifecycle::ServerState,
        listener: TcpListener,
    ) -> (ConnectionAcceptor, Self) {
        let (inbound_streams, new_streams) = mpsc::unbounded();

        (
            ConnectionAcceptor {
                listener,
                inbound_streams,
            },
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
                            Connection {
                                stream: connection.stream,
                                address: connection.address,
                                lifecycle,
                                send_buffer: Default::default(),
                                serializer_buffers: Vec::from_iter((0..16).map(|_| Vec::new())),
                                receive_buffer: Vec::new(),
                                receive_buffer_length: 0,
                                deserializer,
                                serializer,
                                work_in_progress: Default::default(),
                                readable: false,
                                writable: false,
                            },
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
        self.poll_backoff = max(Duration::from_millis(1), self.poll_backoff / 2);
    }

    fn decrease_poll_rate(&mut self) {
        self.poll_backoff = min(
            Duration::from_millis(1000),
            self.poll_backoff + Duration::from_millis(10),
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
            match connection.read_buffer() {
                Ok(false) => {
                    // log::trace!("checked inbound connection buffer");
                }
                Ok(true) => {
                    log::trace!("remote closed connection");
                    drop_connections.push(*token);
                }
                Err(e) => {
                    log::warn!("dropping connection after read: {e:?}");
                    drop_connections.push(*token);
                }
            }
            while !connection.serializer_buffers.is_empty() {
                match pin!(&mut connection.work_in_progress).poll_next(context) {
                    std::task::Poll::Ready(Some(message)) => {
                        let mut buffer = connection
                            .serializer_buffers
                            .pop()
                            .expect("already filtered for connections with room");
                        connection.serializer.encode(message, &mut buffer);
                        log::trace!(
                            "completed message response and enqueueing response buffer: {}b",
                            buffer.len()
                        );

                        // queue up a writev
                        connection.send_buffer.push_back(buffer);
                        // try to get more complete messages for this connection, if any exist
                        continue;
                    }
                    std::task::Poll::Ready(None) => {
                        // log::trace!("drove all tasks for connection to completion");
                        break;
                    }
                    std::task::Poll::Pending => {
                        log::trace!("all tasks for connection are pending");
                        break;
                    }
                }
            }
            if let Err(e) = connection.write_buffers() {
                log::warn!("dropping connection: {e:?}");
                drop_connections.push(*token);
                continue;
            }
        }
        for connection in drop_connections {
            if let Some(mut connection) = self.connections.remove(&connection) {
                let _ = self
                    .mio_io
                    .as_mut()
                    .expect("mio must exist")
                    .poll
                    .registry()
                    .deregister(&mut connection.stream);
            }
        }
    }
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

pub(crate) struct ConnectionAcceptor {
    listener: TcpListener,
    inbound_streams: mpsc::UnboundedSender<NewConnection>,
}

impl ConnectionAcceptor {
    pub fn accept(&self) -> Result<()> {
        let mut connection = match self.listener.accept() {
            Ok((stream, address)) => NewConnection { stream, address },
            Err(e) if would_block(&e) => {
                // I guess there isn't anything to accept after all
                return Ok(());
            }
            Err(e) => {
                // If it was any other kind of error, something went
                // wrong and we terminate with an error.
                return Err(e.into());
            }
        };
        if let Err(e) = connection.stream.set_nodelay(true) {
            log::warn!("could not set nodelay: {e:?}");
        }
        log::debug!("accepted connection {connection:?}");
        self.inbound_streams
            .unbounded_send(connection)
            .map_err(|_e| Error::Dead("connection server"))
    }
}

fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

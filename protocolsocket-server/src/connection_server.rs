use std::collections::HashMap;
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
}

pub trait ConnectionLifecycle: Unpin {
    type Deserializer: Unpin;
    type Serializer: Unpin;
    type ServerState: Unpin;

    /// A new connection lifecycle starts here. If you have a state machine, initialize it here. This is your constructor.
    fn on_connect(server_state: &Self::ServerState) -> Self;
}

/// Once you've configured your connection server the way you want it, execute it on your asynchronous runtime.
pub struct ConnectionServer<Lifecycle: ConnectionLifecycle> {
    new_streams: mpsc::UnboundedReceiver<NewConnection>,
    connection_token_count: usize,
    connections: HashMap<Token, Connection<Lifecycle>>,
    mio_io: Option<Mio>,
    server_state: Lifecycle::ServerState,
}

struct Mio {
    poll: mio::Poll,
    events: mio::Events,
}

impl<Lifecycle: ConnectionLifecycle> ConnectionServer<Lifecycle> {
    pub(crate) fn new(server_state: Lifecycle::ServerState, listener: TcpListener) -> (ConnectionAcceptor, Self) {
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
                }.into(),
                server_state,
            },
        )
    }

    fn poll_register_new_connections(mut self: std::pin::Pin<&mut Self>, context: &mut std::task::Context<'_>) -> std::task::Poll<<Self as std::future::Future>::Output> {
        loop {
            break match pin!(&mut self.new_streams).poll_next(context) {
                std::task::Poll::Ready(Some(mut connection)) => {
                    let token = Token(self.connection_token_count);
                    self.connection_token_count += 1;

                    if let Err(e) = self.mio_io.as_mut().expect("must have mio").poll.registry().register(&mut connection.stream, token, Interest::READABLE.add(Interest::WRITABLE)) {
                        log::error!("failed to register stream: {e:?}");
                        std::task::Poll::Ready(())
                    } else {
                        let lifecycle = Lifecycle::on_connect(&self.server_state);
                        self.connections.insert(token, Connection { stream: connection.stream, address: connection.address, lifecycle });
                        continue
                    }
                }
                std::task::Poll::Ready(None) => {
                    log::warn!("listener closed");
                    return std::task::Poll::Ready(())
                }
                std::task::Poll::Pending => std::task::Poll::Pending,
            }
        }
    }

    fn poll_mio(&mut self, context: &mut std::task::Context<'_>, poll: &mut mio::Poll, events: &mut mio::Events) -> std::task::Poll<<Self as std::future::Future>::Output> {
        // FIXME: schedule this task to wake up again in a smarter way. This just makes sure events aren't missed.....
        context.waker().wake_by_ref();
        if let Err(e) = poll.poll(events, Some(Duration::from_millis(4))) {
            log::error!("failed to poll connections: {e:?}");
            return std::task::Poll::Ready(())
        }

        for event in events.iter() {
            let token = event.token();
            if let Some(connection) = self.connections.get_mut(&token) {
                let done = match handle_connection_event(poll.registry(), connection, event) {
                    Ok(done) => done,
                    Err(e) => {
                        log::error!("failure reading from socket: {e:?}");
                        return std::task::Poll::Ready(())
                    }
                };
                if done {
                    if let Some(mut connection) = self.connections.remove(&token) {
                        match poll.registry().deregister(&mut connection.stream) {
                            Ok(_) => (),
                            Err(e) => {
                                log::error!("could not deregister stream from registry: {e:?}");
                                return std::task::Poll::Ready(())
                            }
                        }
                    }
                }
            } else {
                log::debug!("something happened for a socket that isn't connected anymore {event:?}");
            }
        }
        std::task::Poll::Pending
    }
}

impl<Lifecycle: ConnectionLifecycle> std::future::Future for ConnectionServer<Lifecycle> {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, context: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        if let std::task::Poll::Ready(early_out) = self.as_mut().poll_register_new_connections(context) {
            return std::task::Poll::Ready(early_out)
        }

        let Mio { mut poll, mut events } = self.mio_io.take().expect("state must have mio");
        if let std::task::Poll::Ready(early_out) = self.poll_mio(context, &mut poll, &mut events) {
            return std::task::Poll::Ready(early_out);
        }
        self.mio_io = Mio { poll, events }.into();

        std::task::Poll::Pending
    }
}

fn handle_connection_event(
    registry: &mio::Registry,
    connection: &mut Connection<impl ConnectionLifecycle>,
    event: &mio::event::Event,
) -> std::io::Result<bool> {
    if event.is_writable() {
        // We can (maybe) write to the connection.
        match connection.stream.write(b"wat") {
            // We want to write the entire `DATA` buffer in a single go. If we
            // write less we'll return a short write error (same as
            // `io::Write::write_all` does).
            Ok(n) if n < b"wat".len() => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(_) => {
                // After we've written something we'll reregister the connection
                // to only respond to readable events.
                registry.reregister(&mut connection.stream, event.token(), Interest::READABLE)?
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {}
            // Got interrupted (how rude!), we'll try again.
            Err(ref err) if interrupted(err) => {
                return handle_connection_event(registry, connection, event)
            }
            // Other errors we'll consider fatal.
            Err(err) => return Err(err),
        }
    }

    if event.is_readable() {
        let mut connection_closed = false;
        let mut received_data = vec![0; 4096];
        let mut bytes_read = 0;
        // We can (maybe) read from the connection.
        loop {
            match connection.stream.read(&mut received_data[bytes_read..]) {
                Ok(0) => {
                    // Reading 0 bytes means the other side has closed the
                    // connection or is done writing, then so are we.
                    connection_closed = true;
                    break;
                }
                Ok(n) => {
                    bytes_read += n;
                    if bytes_read == received_data.len() {
                        received_data.resize(received_data.len() + 1024, 0);
                    }
                }
                // Would block "errors" are the OS's way of saying that the
                // connection is not actually ready to perform this I/O operation.
                Err(ref err) if would_block(err) => break,
                Err(ref err) if interrupted(err) => continue,
                // Other errors we'll consider fatal.
                Err(err) => return Err(err),
            }
        }

        if bytes_read != 0 {
            let received_data = &received_data[..bytes_read];
            if let Ok(str_buf) = std::str::from_utf8(received_data) {
                println!("Received data: {}", str_buf.trim_end());
            } else {
                println!("Received (none UTF-8) data: {:?}", received_data);
            }
        }

        if connection_closed {
            println!("Connection closed");
            return Ok(true);
        }
    }

    Ok(false)
}


pub(crate) struct ConnectionAcceptor {
    listener: TcpListener,
    inbound_streams: mpsc::UnboundedSender<NewConnection>,
}

impl ConnectionAcceptor {
    pub fn accept(&self) -> Result<()> {
        let connection = match self.listener.accept() {
            Ok((stream, address)) => {
                NewConnection { stream, address }
            }
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
        log::debug!("accepted connection {connection:?}");
        self.inbound_streams.unbounded_send(connection).map_err(|e| Error::Dead("connection server"))
    }
}

fn would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

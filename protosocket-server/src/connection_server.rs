use std::future::Future;
use std::io::Error;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Context;
use std::task::Poll;

use protosocket::Connection;
use protosocket::ConnectionBindings;
use protosocket::Serializer;
use tokio::sync::mpsc;

pub trait ServerConnector: Unpin {
    type Bindings: ConnectionBindings;

    fn serializer(&self) -> <Self::Bindings as ConnectionBindings>::Serializer;
    fn deserializer(&self) -> <Self::Bindings as ConnectionBindings>::Deserializer;

    fn new_reactor(
        &self,
        optional_outbound: mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
    ) -> <Self::Bindings as ConnectionBindings>::Reactor;

    fn maximum_message_length(&self) -> usize {
        4 * (2 << 20)
    }

    fn max_queued_outbound_messages(&self) -> usize {
        256
    }

    fn connect(
        &self,
        stream: tokio::net::TcpStream,
    ) -> <Self::Bindings as ConnectionBindings>::Stream;
}

/// A `protosocket::Connection` is an IO driver. It directly uses tokio's io wrapper of mio to poll
/// the OS's io primitives, manages read and write buffers, and vends messages to & from connections.
/// Connections send messages to the ConnectionServer through an mpsc channel, and they receive
/// inbound messages via a reactor callback.
///
/// Protosockets are monomorphic messages: You can only have 1 kind of message per service.
/// The expected way to work with this is to use prost and protocol buffers to encode messages.
/// Of course you can do whatever you want, as the telnet example shows.
///
/// Protosocket messages are not opinionated about request & reply. If you are, you will need
/// to implement such a thing. This allows you freely choose whether you want to send
/// fire-&-forget messages sometimes; however it requires you to write your protocol's rules.
/// You get an inbound iterable of <MessageIn> batches and an outbound stream of <MessageOut> per
/// connection - you decide what those mean for you!
///
/// A ProtosocketServer is a future: You spawn it and it runs forever.
pub struct ProtosocketServer<Connector: ServerConnector> {
    connector: Connector,
    listener: tokio::net::TcpListener,
    max_buffer_length: usize,
    max_queued_outbound_messages: usize,
    runtime: tokio::runtime::Handle,
}

impl<Connector: ServerConnector> ProtosocketServer<Connector> {
    /// Construct a new `ProtosocketServer` listening on the provided address.
    /// The address will be bound and listened upon with `SO_REUSEADDR` set.
    /// The server will use the provided runtime to spawn new tcp connections as `protosocket::Connection`s.
    pub async fn new(
        address: std::net::SocketAddr,
        runtime: tokio::runtime::Handle,
        connector: Connector,
    ) -> crate::Result<Self> {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(Arc::new)?;
        Ok(Self {
            connector,
            listener,
            max_buffer_length: 16 * (2 << 20),
            max_queued_outbound_messages: 128,
            runtime,
        })
    }

    /// Set the maximum buffer length for connections created by this server after the setting is applied.
    pub fn set_max_buffer_length(&mut self, max_buffer_length: usize) {
        self.max_buffer_length = max_buffer_length;
    }

    /// Set the maximum queued outbound messages for connections created by this server after the setting is applied.
    pub fn set_max_queued_outbound_messages(&mut self, max_queued_outbound_messages: usize) {
        self.max_queued_outbound_messages = max_queued_outbound_messages;
    }
}

impl<Connector: ServerConnector> Future for ProtosocketServer<Connector> {
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            break match self.listener.poll_accept(context) {
                Poll::Ready(result) => match result {
                    Ok((stream, address)) => {
                        stream.set_nodelay(true)?;
                        let (outbound_submission_queue, outbound_messages) =
                            mpsc::channel(self.max_queued_outbound_messages);
                        let reactor = self
                            .connector
                            .new_reactor(outbound_submission_queue.clone());
                        let stream = self.connector.connect(stream);
                        let connection: Connection<Connector::Bindings> = Connection::new(
                            stream,
                            address,
                            self.connector.deserializer(),
                            self.connector.serializer(),
                            self.max_buffer_length,
                            self.max_queued_outbound_messages,
                            outbound_messages,
                            reactor,
                        );
                        self.runtime.spawn(connection);
                        continue;
                    }
                    Err(e) => {
                        log::error!("failed to accept connection: {e:?}");
                        continue;
                    }
                },
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

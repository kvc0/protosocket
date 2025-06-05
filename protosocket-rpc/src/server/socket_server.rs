use std::future::Future;
use std::io::Error;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;

use protosocket::Connection;
use tokio::sync::mpsc;

use super::connection_server::RpcConnectionServer;
use super::rpc_submitter::RpcSubmitter;
use super::server_traits::SocketService;

/// A `SocketRpcServer` is a server future. It listens on a socket and spawns new connections,
/// with a ConnectionService to handle each connection.
///
/// Protosockets use monomorphic messages: You can only have 1 kind of message per service.
/// The expected way to work with this is to use prost and protocol buffers to encode messages.
///
/// The socket server hosts your SocketService.
/// Your SocketService creates a ConnectionService for each new connection.
/// Your ConnectionService manages one connection. It is Dropped when the connection is closed.
pub struct SocketRpcServer<TSocketService>
where
    TSocketService: SocketService,
{
    socket_server: TSocketService,
    listener: tokio::net::TcpListener,
    max_buffer_length: usize,
    max_queued_outbound_messages: usize,
}

impl<TSocketService> SocketRpcServer<TSocketService>
where
    TSocketService: SocketService,
{
    /// Construct a new `SocketRpcServer` listening on the provided address.
    pub async fn new(
        address: std::net::SocketAddr,
        socket_server: TSocketService,
    ) -> crate::Result<Self> {
        let listener = tokio::net::TcpListener::bind(address).await?;
        Ok(Self {
            socket_server,
            listener,
            max_buffer_length: 16 * (2 << 20),
            max_queued_outbound_messages: 128,
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

impl<TSocketService> Future for SocketRpcServer<TSocketService>
where
    TSocketService: SocketService,
{
    type Output = Result<(), Error>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            break match self.listener.poll_accept(context) {
                Poll::Ready(result) => match result {
                    Ok((stream, address)) => {
                        stream.set_nodelay(true)?;
                        let (submitter, inbound_messages) = RpcSubmitter::new();
                        let (outbound_messages, outbound_messages_receiver) =
                            mpsc::channel(self.max_queued_outbound_messages);
                        let connection_service = self.socket_server.new_connection_service(address);
                        let connection_rpc_server = RpcConnectionServer::new(
                            connection_service,
                            inbound_messages,
                            outbound_messages,
                        );
                        let deserializer = self.socket_server.deserializer();
                        let serializer = self.socket_server.serializer();
                        let max_buffer_length = self.max_buffer_length;
                        let max_queued_outbound_messages = self.max_queued_outbound_messages;

                        let stream_future = self.socket_server.connect_stream(stream);

                        tokio::spawn(async move {
                            match stream_future.await {
                                Ok(stream) => {
                                    let connection: Connection<RpcSubmitter<TSocketService>> =
                                        Connection::new(
                                            stream,
                                            address,
                                            deserializer,
                                            serializer,
                                            max_buffer_length,
                                            max_queued_outbound_messages,
                                            outbound_messages_receiver,
                                            submitter,
                                        );
                                    tokio::spawn(connection);
                                    tokio::spawn(connection_rpc_server);
                                }
                                Err(e) => {
                                    log::error!("failed to connect stream: {e:?}");
                                }
                            }
                        });

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

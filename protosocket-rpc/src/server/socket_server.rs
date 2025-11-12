use protosocket::Connection;
use protosocket::Encoder;
use protosocket::SocketListener;
use protosocket::SocketResult;
use std::future::Future;
use std::io::Error;
use std::pin::Pin;
use std::task::Context;
use std::task::Poll;
use tokio::sync::mpsc;

use crate::server::server_traits::SpawnConnection;
use crate::server::server_traits::TokioSpawnConnection;
use crate::server::ConnectionService;

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
pub struct SocketRpcServer<TSocketService, TSpawnConnection>
where
    TSocketService: SocketService,
{
    socket_server: TSocketService,
    spawner: TSpawnConnection,
    listener: TSocketService::SocketListener,
    max_buffer_length: usize,
    buffer_allocation_increment: usize,
    max_queued_outbound_messages: usize,
}

impl<TSocketService> SocketRpcServer<TSocketService, TokioSpawnConnection<TSocketService>>
where
    TSocketService: SocketService,
    TSocketService::RequestDecoder: Send,
    TSocketService::ResponseEncoder: Send,
    <TSocketService::ResponseEncoder as Encoder>::Serialized: Send,
    <TSocketService::SocketListener as SocketListener>::Stream: Send,
    <TSocketService::ConnectionService as ConnectionService>::UnaryFutureType: Send,
    <TSocketService::ConnectionService as ConnectionService>::StreamType: Send,
{
    /// Construct a new `SocketRpcServer` with a listener.
    ///
    /// This assumes a Tokio runtime, and is only available when your `SocketService` is
    /// transitively `Send`.
    pub async fn new(
        listener: TSocketService::SocketListener,
        socket_server: TSocketService,
        max_buffer_length: usize,
        buffer_allocation_increment: usize,
        max_queued_outbound_messages: usize,
    ) -> crate::Result<Self> {
        Self::new_with_spawner(
            listener,
            socket_server,
            max_buffer_length,
            buffer_allocation_increment,
            max_queued_outbound_messages,
            TokioSpawnConnection::default(),
        )
        .await
    }
}

impl<TSocketService, TSpawnConnection> SocketRpcServer<TSocketService, TSpawnConnection>
where
    TSocketService: SocketService,
    TSpawnConnection: SpawnConnection<TSocketService>,
{
    /// Construct a new `SocketRpcServer` with a listener and a spawner.
    #[allow(clippy::too_many_arguments)]
    pub async fn new_with_spawner(
        listener: TSocketService::SocketListener,
        socket_server: TSocketService,
        max_buffer_length: usize,
        buffer_allocation_increment: usize,
        max_queued_outbound_messages: usize,
        spawner: TSpawnConnection,
    ) -> crate::Result<Self> {
        Ok(Self {
            socket_server,
            spawner,
            listener,
            max_buffer_length,
            buffer_allocation_increment,
            max_queued_outbound_messages,
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

impl<TSocketService, TSpawnConnection> Unpin for SocketRpcServer<TSocketService, TSpawnConnection> where
    TSocketService: SocketService
{
}
impl<TSocketService, TSpawnConnection> Future for SocketRpcServer<TSocketService, TSpawnConnection>
where
    TSocketService: SocketService,
    TSpawnConnection: SpawnConnection<TSocketService>,
{
    type Output = Result<(), Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            break match self.listener.poll_accept(context) {
                Poll::Ready(result) => match result {
                    SocketResult::Stream(stream) => {
                        let connection_service = self.socket_server.new_stream_service(&stream);
                        let (submitter, inbound_messages) = RpcSubmitter::new();
                        let (outbound_messages, outbound_messages_receiver) =
                            mpsc::channel(self.max_queued_outbound_messages);
                        let connection_rpc_server = RpcConnectionServer::new(
                            connection_service,
                            inbound_messages,
                            outbound_messages,
                        );
                        let connection: Connection<
                            <TSocketService::SocketListener as SocketListener>::Stream,
                            TSocketService::RequestDecoder,
                            TSocketService::ResponseEncoder,
                            RpcSubmitter<TSocketService>,
                        > = Connection::new(
                            stream,
                            self.socket_server.decoder(),
                            self.socket_server.encoder(),
                            self.max_buffer_length,
                            self.buffer_allocation_increment,
                            self.max_queued_outbound_messages,
                            outbound_messages_receiver,
                            submitter,
                        );
                        self.spawner
                            .spawn_connection(connection, connection_rpc_server);
                        continue;
                    }
                    SocketResult::Disconnect => Poll::Ready(Ok(())),
                },
                Poll::Pending => {
                    // hooray, listener is pending.
                    Poll::Pending
                }
            };
        }
    }
}

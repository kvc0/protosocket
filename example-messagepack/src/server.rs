use std::{future::Future, net::SocketAddr, sync::atomic::AtomicUsize};

use futures::{future::BoxFuture, stream::BoxStream, FutureExt, Stream, StreamExt};
use messages::{EchoRequest, EchoResponse, EchoStream, Request, Response, ResponseBehavior};
use protosocket_rpc::{
    server::{ConnectionService, RpcKind, SocketService, TcpSocketListener},
    ProtosocketControlCode,
};
use tokio::net::TcpStream;

mod messages;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    static I: AtomicUsize = AtomicUsize::new(0);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .thread_name_fn(|| {
            format!(
                "app-{}",
                I.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        })
        .worker_threads(2)
        .event_interval(7)
        .enable_all()
        .build()?;

    runtime.block_on(run_main())
}

#[allow(clippy::expect_used)]
async fn run_main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let mut server = protosocket_rpc::server::SocketRpcServer::new(
        TcpSocketListener::listen(
            std::env::var("HOST")
                .unwrap_or_else(|_| "0.0.0.0:9000".to_string())
                .parse()?,
            1024,
            None,
        )?,
        DemoRpcSocketService,
        4 << 20,
        1 << 20,
        128,
    )
    .await?;
    server.set_max_queued_outbound_messages(512);

    tokio::spawn(server).await??;
    Ok(())
}

/// This is the service that will be used to handle new connections.
/// It doesn't do much; yours might be simple like this too, or it might wire your per-connection
/// ConnectionServices to application-wide state tracking.
struct DemoRpcSocketService;
impl SocketService for DemoRpcSocketService {
    type RequestDecoder = protosocket_messagepack::ProtosocketMessagePackDecoder<Request>;
    type ResponseEncoder = protosocket_messagepack::MessagePackSerializer<Response>;
    type ConnectionService = DemoRpcConnectionServer;
    type Stream = TcpStream;
    type SocketListener = TcpSocketListener;

    fn decoder(&self) -> Self::RequestDecoder {
        Self::RequestDecoder::default()
    }

    fn encoder(&self) -> Self::ResponseEncoder {
        Self::ResponseEncoder::default()
    }

    fn accept_stream(
        &self,
        (stream, address): (TcpStream, SocketAddr),
    ) -> impl Future<Output = std::io::Result<(Self::Stream, Self::ConnectionService)>> + Send + 'static
    {
        log::info!("new connection server {address}");
        futures::future::ok((stream, DemoRpcConnectionServer { address }))
    }
}

/// This is the entry point for each Connection. State per-connection is tracked, and you
/// get mutable access to the service on each new rpc for state tracking.
struct DemoRpcConnectionServer {
    address: std::net::SocketAddr,
}
impl ConnectionService for DemoRpcConnectionServer {
    type Request = Request;
    type Response = Response;
    // Ideally you'd use real Future and Stream types here for performance and debuggability.
    // For a demo though, it's fine to use BoxFuture and BoxStream.
    type UnaryFutureType = BoxFuture<'static, Response>;
    type StreamType = BoxStream<'static, Response>;

    fn new_rpc(
        &mut self,
        initiating_message: Self::Request,
    ) -> RpcKind<Self::UnaryFutureType, Self::StreamType> {
        log::debug!("{} new rpc: {initiating_message:?}", self.address);
        let request_id = initiating_message.request_id;
        let behavior = initiating_message.response_behavior;
        match initiating_message.body {
            Some(echo) => match behavior {
                ResponseBehavior::Unary => RpcKind::Unary(echo_request(request_id, echo).boxed()),
                ResponseBehavior::Stream => {
                    RpcKind::Streaming(echo_stream(request_id, echo).boxed())
                }
            },
            None => {
                // No completion messages will be sent for this message
                log::warn!(
                    "{request_id} no request in rpc body. This may cause a client memory leak."
                );
                RpcKind::Unknown
            }
        }
    }
}

async fn echo_request(request_id: u64, echo: EchoRequest) -> Response {
    Response {
        request_id,
        code: ProtosocketControlCode::Normal as u32,
        kind: Some(messages::EchoResponseKind::Echo(EchoResponse {
            message: echo.message,
            nanotime: echo.nanotime,
        })),
    }
}

fn echo_stream(request_id: u64, echo: EchoRequest) -> impl Stream<Item = Response> {
    let nanotime = echo.nanotime;
    futures::stream::iter(echo.message.into_bytes().into_iter().enumerate().map(
        move |(sequence, c)| Response {
            request_id,
            code: ProtosocketControlCode::Normal as u32,
            kind: Some(messages::EchoResponseKind::Stream(EchoStream {
                message: (c as char).to_string(),
                nanotime,
                sequence: sequence as u64,
            })),
        },
    ))
}

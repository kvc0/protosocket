use futures::{Stream, StreamExt, future::join_all, stream::BoxStream};
use messages::{EchoRequest, EchoResponse, EchoStream, Request, Response, ResponseBehavior};
use protosocket::{PooledEncoder, StreamWithAddress, TcpSocketListener};
use protosocket_prost::{ProstDecoder, ProstSerializer};
use protosocket_rpc::{
    ProtosocketControlCode,
    server::{ConnectionService, LevelSpawn, RpcKind, SocketService},
};
use tokio::net::TcpStream;

mod messages;
mod tracing_stuff;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = level_runtime::Builder::default()
        .thread_name_prefix("server")
        .worker_threads(2)
        .event_interval(3)
        .global_queue_interval(7)
        .enable_all()
        .build();
    runtime.set_default();
    runtime.handle().spawn_balanced(run_main());

    runtime.run();
    Ok(())
}

#[allow(clippy::expect_used)]
async fn run_main() -> Result<(), std::io::Error> {
    env_logger::init();
    tracing_stuff::set_default_tracer("proto-server");

    let futures = join_all(level_runtime::spawn_on_each(|| async move {
        let mut server = protosocket_rpc::server::SocketRpcServer::new_with_spawner(
            TcpSocketListener::listen(
                std::env::var("HOST")
                    .unwrap_or_else(|_| "0.0.0.0:9000".to_string())
                    .parse()
                    .expect("must be able to parse listen address"),
                1,
                None,
            )?,
            DemoRpcSocketService,
            4 << 20,
            1 << 20,
            128,
            LevelSpawn::default(),
        )
        .expect("must be able to listen");
        server.set_max_queued_outbound_messages(512);
        server.await
    }));
    futures.await;

    Ok(())
}

/// This is the service that will be used to handle new connections.
/// It doesn't do much; yours might be simple like this too, or it might wire your per-connection
/// ConnectionServices to application-wide state tracking.
struct DemoRpcSocketService;
impl SocketService for DemoRpcSocketService {
    type ConnectionService = DemoRpcConnectionServer;
    type SocketListener = TcpSocketListener;

    fn codec(&self) -> <Self::ConnectionService as ConnectionService>::Codec {
        (
            PooledEncoder::new_with_pool_size(64, Default::default()),
            ProstDecoder::default(),
        )
    }

    fn new_stream_service(&self, stream: &StreamWithAddress<TcpStream>) -> Self::ConnectionService {
        log::info!("new connection server {}", stream.address());
        DemoRpcConnectionServer {
            address: stream.address(),
        }
    }
}

/// This is the entry point for each Connection. State per-connection is tracked, and you
/// get mutable access to the service on each new rpc for state tracking.
///
/// The connection drives your rpcs: they are only polled when the connection can send,
/// so a slow peer slows its rpcs down instead of buffering responses without bound.
struct DemoRpcConnectionServer {
    address: std::net::SocketAddr,
}
impl ConnectionService for DemoRpcConnectionServer {
    type Codec = (
        // Use a pooled encoder to amortize memory allocation cost.
        // Each connection gets its own little memory pool.
        PooledEncoder<ProstSerializer<Response>>,
        ProstDecoder<Request>,
    );
    type Request = Request;
    type Response = Response;
    type UnaryFutureType = futures::future::Ready<Response>;
    type StreamType = BoxStream<'static, Response>;

    fn new_rpc(
        &mut self,
        initiating_message: Self::Request,
    ) -> RpcKind<Self::UnaryFutureType, Self::StreamType> {
        log::debug!("{} new rpc: {initiating_message:?}", self.address);
        let request_id = initiating_message.request_id;
        let behavior = initiating_message.response_behavior();
        match initiating_message.body {
            Some(echo) => match behavior {
                ResponseBehavior::Unary => RpcKind::Unary(futures::future::ready(
                    immediate_echo_response(request_id, echo),
                )),
                ResponseBehavior::Stream => {
                    RpcKind::Streaming(echo_stream(request_id, echo).boxed())
                }
            },
            None => {
                log::warn!("received empty echo request id {request_id}");
                RpcKind::Cancelled
            }
        }
    }
}

fn immediate_echo_response(request_id: u64, echo: EchoRequest) -> Response {
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

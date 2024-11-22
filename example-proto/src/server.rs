use futures::{future::BoxFuture, stream::BoxStream, FutureExt, Stream, StreamExt};
use messages::{EchoRequest, EchoResponse, EchoStream, Request, Response, ResponseBehavior};
use protosocket_prost::ProstSerializer;
use protosocket_rpc::{
    server::{ConnectionService, RpcKind, SocketService},
    ProtosocketControlCode,
};

mod messages;

#[allow(clippy::expect_used)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let server = protosocket_rpc::server::SocketRpcServer::new(
        std::env::var("HOST")
            .unwrap_or_else(|_| "0.0.0.0:9000".to_string())
            .parse()?,
        DemoRpcSocketService,
    )
    .await?;

    tokio::spawn(server).await??;
    Ok(())
}

/// This is the service that will be used to handle new connections.
/// It doesn't do much; yours might be simple like this too, or it might wire your per-connection
/// ConnectionServices to application-wide state tracking.
struct DemoRpcSocketService;
impl SocketService for DemoRpcSocketService {
    type RequestDeserializer = ProstSerializer<Request, Response>;
    type ResponseSerializer = ProstSerializer<Request, Response>;
    type ConnectionService = DemoRpcConnectionServer;

    fn deserializer(&self) -> Self::RequestDeserializer {
        Self::RequestDeserializer::default()
    }

    fn serializer(&self) -> Self::ResponseSerializer {
        Self::ResponseSerializer::default()
    }

    fn new_connection_service(&self, address: std::net::SocketAddr) -> Self::ConnectionService {
        log::info!("new connection server {address}");
        DemoRpcConnectionServer { address }
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
        let behavior = initiating_message.response_behavior();
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

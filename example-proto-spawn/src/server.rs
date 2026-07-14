//! A protosocket-rpc server that spawns its rpcs onto the runtime.
//!
//! Rpcs are normally polled by the connection, which stops polling them when the peer
//! can't receive. Spawning decouples rpc work from that backpressure: the spawned task
//! runs regardless. Use a completion future for unary work, and a producer task behind
//! a bounded channel for streams - the channel capacity is how far the producer can run
//! ahead of the peer.

use std::pin::pin;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use futures::{future::BoxFuture, stream::BoxStream, FutureExt, Stream, StreamExt};
use messages::{EchoRequest, EchoResponse, EchoStream, Request, Response, ResponseBehavior};
use protosocket::{PooledEncoder, StreamWithAddress, TcpSocketListener};
use protosocket_prost::{ProstDecoder, ProstSerializer};
use protosocket_rpc::{
    server::{ConnectionService, RpcKind, SocketService},
    Message, ProtosocketControlCode,
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
    )?;
    server.set_max_queued_outbound_messages(512);

    tokio::spawn(server).await??;
    Ok(())
}

struct DemoRpcSocketService;
impl SocketService for DemoRpcSocketService {
    type Codec = (
        PooledEncoder<ProstSerializer<Response>>,
        ProstDecoder<Request>,
    );
    type ConnectionService = DemoRpcConnectionServer;
    type SocketListener = TcpSocketListener;

    fn codec(&self) -> Self::Codec {
        Default::default()
    }

    fn new_stream_service(&self, stream: &StreamWithAddress<TcpStream>) -> Self::ConnectionService {
        log::info!("new connection server {}", stream.address());
        DemoRpcConnectionServer {
            address: stream.address(),
        }
    }
}

struct DemoRpcConnectionServer {
    address: std::net::SocketAddr,
}
impl ConnectionService for DemoRpcConnectionServer {
    type Request = Request;
    type Response = Response;
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
                ResponseBehavior::Unary => {
                    // Spawn the work and return a future that completes with the task.
                    let work = tokio::spawn(echo_request(request_id, echo));
                    RpcKind::Unary(
                        async move {
                            match work.await {
                                Ok(response) => response,
                                Err(join_error) => {
                                    log::error!("rpc task failed: {join_error}");
                                    Response::cancelled(request_id)
                                }
                            }
                        }
                        .boxed(),
                    )
                }
                ResponseBehavior::Stream => {
                    // Spawn the producer behind a bounded channel. The producer waits
                    // when the channel is full, and quits when the rpc is cancelled or
                    // the connection closes (the receiver drops).
                    let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
                    tokio::spawn(async move {
                        let mut stream = pin!(echo_stream(request_id, echo));
                        while let Some(response) = stream.next().await {
                            if sender.send(response).await.is_err() {
                                break;
                            }
                        }
                    });
                    RpcKind::Streaming(
                        futures::stream::poll_fn(move |context| receiver.poll_recv(context))
                            .boxed(),
                    )
                }
            },
            None => {
                log::warn!("received empty echo request id {request_id}");
                RpcKind::Cancelled
            }
        }
    }
}

async fn echo_request(request_id: u64, echo: EchoRequest) -> Response {
    // Pretend this is compute-heavy work that deserves its own task.
    tokio::time::sleep(Duration::from_micros(1)).await;
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

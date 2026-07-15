//! A protosocket-rpc server that spawns its unary rpcs onto the runtime.
//!
//! Rpcs are normally polled by the connection, which stops polling them when the peer
//! can't receive. Spawning decouples rpc work from that backpressure: the spawned task
//! runs regardless. Bound what you spawn - here, rpcs shed load when the connection's
//! spawn slots are all in flight.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use futures::{future::BoxFuture, stream::BoxStream, FutureExt};
use messages::{EchoRequest, EchoResponse, Request, Response};
use protosocket::{PooledEncoder, StreamWithAddress, TcpSocketListener};
use protosocket_prost::{ProstDecoder, ProstSerializer};
use protosocket_rpc::{
    server::{ConnectionService, RpcKind, SocketService},
    Message, ProtosocketControlCode,
};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

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
    type ConnectionService = DemoRpcConnectionServer;
    type SocketListener = TcpSocketListener;

    fn codec(&mut self) -> <Self::ConnectionService as ConnectionService>::Codec {
        Default::default()
    }

    fn new_stream_service(
        &mut self,
        stream: &StreamWithAddress<TcpStream>,
    ) -> Self::ConnectionService {
        log::info!("new connection server {}", stream.address());
        DemoRpcConnectionServer {
            address: stream.address(),
            // How many spawned unary rpcs may be in flight for this connection. Small
            // enough that the example client can outrun it and show shedding.
            spawn_limit: Arc::new(Semaphore::new(16)),
        }
    }
}

struct DemoRpcConnectionServer {
    address: std::net::SocketAddr,
    spawn_limit: Arc<Semaphore>,
}
impl ConnectionService for DemoRpcConnectionServer {
    type Codec = (
        PooledEncoder<ProstSerializer<Response>>,
        ProstDecoder<Request>,
    );
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
        match initiating_message.body {
            Some(echo) => {
                // Spawning opts this work out of connection backpressure, so bound it
                // by shedding: no slot, no rpc. Slots are held until responses are
                // handed off, so a peer that stops receiving sheds instead of
                // accumulating spawned work. A real service might prefer an explicit
                // "too busy" response over a cancellation.
                match Arc::clone(&self.spawn_limit).try_acquire_owned() {
                    Ok(slot) => {
                        let work = tokio::spawn(echo_request(request_id, echo));
                        RpcKind::Unary(
                            async move {
                                let _held_until_handoff = slot;
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
                    Err(_no_slot) => RpcKind::Cancelled,
                }
            }
            None => {
                log::warn!("received empty echo request id {request_id}");
                RpcKind::Cancelled
            }
        }
    }
}

async fn echo_request(request_id: u64, echo: EchoRequest) -> Response {
    // Pretend this is heavy work that deserves its own task. It is slow enough that
    // the example client outruns the spawn slots, so you can watch shedding happen.
    tokio::time::sleep(Duration::from_micros(500)).await;
    Response {
        request_id,
        code: ProtosocketControlCode::Normal as u32,
        body: Some(EchoResponse {
            message: echo.message,
            nanotime: echo.nanotime,
        }),
    }
}

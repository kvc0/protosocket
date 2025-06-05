use std::{
    future::Future,
    sync::{atomic::AtomicUsize, Arc},
};

use futures::{future::BoxFuture, stream::BoxStream, FutureExt, Stream, StreamExt};
use messages::{EchoRequest, EchoResponse, EchoStream, Request, Response, ResponseBehavior};
use protosocket_prost::ProstSerializer;
use protosocket_rpc::{
    server::{ConnectionService, RpcKind, SocketService},
    ProtosocketControlCode,
};
use rustls_pemfile::Item;

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

    let cert =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])?;
    let certpem = cert.cert.pem();
    let certs: Result<Vec<rustls_pki_types::CertificateDer<'static>>, std::io::Error> =
        rustls_pemfile::certs(&mut certpem.clone().into_bytes().as_slice()).collect();
    let certs = certs?;

    let pkpem = cert.key_pair.serialize_pem();
    let mut keys: Vec<rustls_pki_types::PrivateKeyDer<'static>> =
        rustls_pemfile::read_all(&mut pkpem.clone().into_bytes().as_slice())
            .filter_map(|item| item.ok())
            .filter_map(|item| match item {
                Item::X509Certificate(_) => {
                    log::error!("unknown unsupported x509");
                    None
                }
                Item::Pkcs1Key(key) => Some(rustls_pki_types::PrivateKeyDer::Pkcs1(key)),
                Item::Pkcs8Key(key) => Some(rustls_pki_types::PrivateKeyDer::Pkcs8(key)),
                Item::Sec1Key(key) => Some(rustls_pki_types::PrivateKeyDer::Sec1(key)),
                _ => None,
            })
            .collect();

    let server_config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, keys.pop().expect("must have a key"))?;

    let mut server = protosocket_rpc::server::SocketRpcServer::new(
        std::env::var("HOST")
            .unwrap_or_else(|_| "0.0.0.0:9000".to_string())
            .parse()?,
        DemoRpcSocketService {
            tls_acceptor: Arc::new(server_config).into(),
        },
    )
    .await?;
    server.set_max_queued_outbound_messages(512);

    tokio::spawn(server).await??;
    Ok(())
}

/// This is the service that will be used to handle new connections.
/// It doesn't do much; yours might be simple like this too, or it might wire your per-connection
/// ConnectionServices to application-wide state tracking.
struct DemoRpcSocketService {
    tls_acceptor: tokio_rustls::TlsAcceptor,
}
impl SocketService for DemoRpcSocketService {
    type RequestDeserializer = ProstSerializer<Request, Response>;
    type ResponseSerializer = ProstSerializer<Request, Response>;
    type ConnectionService = DemoRpcConnectionServer;
    type Stream = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;

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

    fn connect_stream(
        &self,
        stream: tokio::net::TcpStream,
    ) -> impl Future<Output = std::io::Result<Self::Stream>> + Send + 'static {
        let acceptor = self.tls_acceptor.clone();
        async move { acceptor.accept(stream).await }
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

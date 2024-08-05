use std::{
    net::TcpListener,
    sync::{atomic::AtomicUsize, Arc},
};

use messages::{EchoResponse, Request, Response};
use protosocket::{
    ConnectionBindings, ConnectionDriver, MessageReactor, ReactorStatus, Serializer,
};
use protosocket_prost::{ProstSerializer, ProstServerConnectionBindings};
use protosocket_server::{Server, ServerConnector};
use tokio::sync::Semaphore;

mod messages;

#[allow(clippy::expect_used)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    static I: AtomicUsize = AtomicUsize::new(0);
    let connection_runtime = tokio::runtime::Builder::new_multi_thread()
        .thread_name_fn(|| {
            format!(
                "conn-{}",
                I.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        })
        .worker_threads(2)
        .enable_all()
        .build()?;

    let mut server = Server::new()?;
    let server_context = ServerContext {
        _connections: Default::default(),
        connection_runtime: connection_runtime.handle().clone(),
    };

    let listener = TcpListener::bind("127.0.0.1:9000")?;
    listener.set_nonblocking(true)?;
    let port_nine_thousand = server.register_multithreaded_service_listener::<ServerContext>(
        listener,
        server_context.clone(),
        2,
    )?;

    let listener = std::thread::Builder::new()
        .name("listener".to_string())
        .spawn(move || server.serve().expect("server must serve"))?;
    for io_thread in port_nine_thousand {
        std::thread::Builder::new()
            .name("service-io".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("io thread can have a runtime");
                runtime.block_on(io_thread);
            })?;
    }

    listener.join().expect("listener completes");
    Ok(())
}

#[derive(Clone)]
struct ServerContext {
    _connections: Arc<AtomicUsize>,
    connection_runtime: tokio::runtime::Handle,
}

impl ServerConnector for ServerContext {
    type Bindings = ProstServerConnectionBindings<Request, Response>;
    type Reactor = ProtoReflexReactor;

    fn serializer(&self) -> <Self::Bindings as ConnectionBindings>::Serializer {
        ProstSerializer::default()
    }

    fn deserializer(&self) -> <Self::Bindings as ConnectionBindings>::Deserializer {
        ProstSerializer::default()
    }

    fn take_new_connection(
        &self,
        address: std::net::SocketAddr,
        _outbound: tokio::sync::mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
        connection_driver: ConnectionDriver<Self::Bindings, Self::Reactor>,
    ) {
        log::info!("new connection from {address:?}");
        // The ProtoReflexReactor implements the server for this example server
        self.connection_runtime.spawn(connection_driver);
    }

    fn new_reactor(
        &self,
        optional_outbound: tokio::sync::mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
    ) -> Self::Reactor {
        ProtoReflexReactor {
            outbound: optional_outbound,
            concurrent_requests: Arc::new(Semaphore::new(1024)),
        }
    }
}

struct ProtoReflexReactor {
    outbound: tokio::sync::mpsc::Sender<Response>,
    concurrent_requests: Arc<Semaphore>,
}
impl MessageReactor for ProtoReflexReactor {
    type Inbound = Request;

    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus {
        for message in messages {
            let permit = match self.concurrent_requests.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    log::warn!("shedding load");
                    continue;
                    // could return err here and disconnect the client
                    // return ReactorStatus::Disconnect;
                }
            };
            let outbound = self.outbound.clone();
            tokio::spawn(async move {
                let permit = permit;
                if let Err(e) = outbound
                    .send(Response {
                        request_id: message.request_id,
                        body: message.body.map(|b| EchoResponse { message: b.message }),
                    })
                    .await
                {
                    log::trace!("could not send response: {e:?}")
                }
                log::trace!(
                    "permits at end of request handler: {}",
                    permit.num_permits()
                );
            });
        }
        ReactorStatus::Continue
    }
}

use std::sync::{atomic::AtomicUsize, Arc};

use futures::{stream::FuturesUnordered, StreamExt};
use messages::{EchoResponse, Request, Response};
use protosocket_connection::{ConnectionBindings, Deserializer, Serializer};
use protosocket_prost::{ProstSerializer, ProstServerConnectionBindings};
use protosocket_server::{Server, ServerConnector};

mod messages;

#[allow(clippy::expect_used)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut server = Server::new()?;
    let server_context = ServerContext::default();

    let port_nine_thousand = server
        .register_service_listener::<ServerContext>("127.0.0.1:9000", server_context.clone())?;

    std::thread::spawn(move || server.serve().expect("server must serve"));

    tokio::spawn(port_nine_thousand)
        .await
        .expect("service must serve");

    Ok(())
}

#[derive(Default, Clone)]
struct ServerContext {
    _connections: Arc<AtomicUsize>,
}

impl ServerConnector for ServerContext {
    type Bindings = ProstServerConnectionBindings<Request, Response>;

    fn serializer(&self) -> <Self::Bindings as ConnectionBindings>::Serializer {
        ProstSerializer::default()
    }

    fn deserializer(&self) -> <Self::Bindings as ConnectionBindings>::Deserializer {
        ProstSerializer::default()
    }

    fn take_new_connection(
        &self,
        address: std::net::SocketAddr,
        outbound: tokio::sync::mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
        mut inbound: tokio::sync::mpsc::Receiver<
            <<Self::Bindings as ConnectionBindings>::Deserializer as Deserializer>::Message,
        >,
    ) {
        tokio::spawn(async move {
            log::info!("new connection from {address:?}");

            let mut concurrent_requests = FuturesUnordered::new();

            while let Some(message) = inbound.recv().await {
                let outbound = outbound.clone();
                concurrent_requests.push(tokio::spawn(async move {
                    if let Err(e) = outbound
                        .send(Response {
                            request_id: message.request_id,
                            body: message.body.map(|b| EchoResponse { message: b.message }),
                        })
                        .await
                    {
                        log::error!("could not send response: {e:?}")
                    }
                }));
                while 99 < concurrent_requests.len() {
                    log::trace!("concurrency limiter");
                    concurrent_requests.next().await;
                }
            }
        });
    }
}

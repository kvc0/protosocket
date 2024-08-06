use std::sync::{atomic::AtomicUsize, Arc};

use messages::{EchoResponse, Request, Response};
use protosocket::{ConnectionBindings, MessageReactor, ReactorStatus, Serializer};
use protosocket_prost::{ProstSerializer, ProstServerConnectionBindings};
use protosocket_server::{ProtosocketServer, ServerConnector};
use tokio::sync::Semaphore;

mod messages;

#[allow(clippy::expect_used)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let server_context = ServerContext {
        _connections: Default::default(),
    };
    let server = ProtosocketServer::new(
        std::env::var("HOST")
            .unwrap_or_else(|_| "0.0.0.0:9000".to_string())
            .parse()?,
        tokio::runtime::Handle::current(),
        server_context,
    )
    .await?;

    tokio::spawn(server).await.expect("listener completes");
    Ok(())
}

#[derive(Clone)]
struct ServerContext {
    _connections: Arc<AtomicUsize>,
}

impl ServerConnector for ServerContext {
    type Bindings = ProstServerConnectionBindings<Request, Response, ProtoReflexReactor>;

    fn serializer(&self) -> <Self::Bindings as ConnectionBindings>::Serializer {
        ProstSerializer::default()
    }

    fn deserializer(&self) -> <Self::Bindings as ConnectionBindings>::Deserializer {
        ProstSerializer::default()
    }

    fn new_reactor(
        &self,
        optional_outbound: tokio::sync::mpsc::Sender<
            <<Self::Bindings as ConnectionBindings>::Serializer as Serializer>::Message,
        >,
    ) -> <Self::Bindings as ConnectionBindings>::Reactor {
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
                        body: message.body.map(|b| EchoResponse {
                            message: b.message,
                            nanotime: b.nanotime,
                        }),
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

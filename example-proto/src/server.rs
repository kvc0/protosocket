use std::sync::{atomic::AtomicUsize, Arc};

use futures::{future::BoxFuture, FutureExt};
use messages::{EchoResponse, Request, Response};
use protosocket_prost::{MessageExecutor, ProtocolBufferConnectionBindings};
use protosocket_server::Server;

mod messages;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut server = Server::new()?;
    let server_context = ServerContext::default();

    let port_nine_thousand = server.register_service_listener::<ProtocolBufferConnectionBindings<
        ServerContext,
        Request,
        Response,
        BoxFuture<'static, Response>,
    >>("127.0.0.1:9000", server_context.clone())?;

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

impl MessageExecutor<Request, BoxFuture<'static, Response>> for ServerContext {
    fn execute(&mut self, message: Request) -> BoxFuture<'static, Response> {
        async move {
            Response {
                request_id: message.request_id,
                body: message.body.map(|body| EchoResponse {
                    message: body.message,
                }),
            }
        }
        .boxed()
    }
}

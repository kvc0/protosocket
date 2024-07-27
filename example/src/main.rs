use std::sync::{atomic::AtomicUsize, Arc};

use protocolsocket_server::ConnectionLifecycle;


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut server = protocolsocket_server::Server::new()?;
    let server_context = ServerContext::default();
    let port_nine_thousand = server.register_service_listener::<Connection>("127.0.0.1:9000", server_context.clone())?;

    std::thread::spawn(move || { server.serve().expect("server must serve") });

    tokio::spawn(port_nine_thousand).await.expect("service must serve");
    Ok(())
}

#[derive(Default, Clone)]
struct ServerContext {
    connections: Arc<AtomicUsize>
}

struct Connection {
    seen: usize,
}

impl ConnectionLifecycle for Connection {
    type Deserializer = ();
    type Serializer = ();
    type ServerState = ServerContext;

    fn on_connect(server_state: &Self::ServerState) -> Self {
        let seen = server_state.connections.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        log::info!("connected {seen}");
        Self {
            seen: 0
        }
    }
}

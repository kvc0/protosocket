use std::{
    io::Read,
    sync::{atomic::AtomicUsize, Arc},
    time::Duration,
};

use futures::{future::BoxFuture, FutureExt, TryFutureExt};
use protosocket_server::{ConnectionLifecycle, DeserializeError, Deserializer, Serializer, Server};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut server = Server::new()?;
    let server_context = ServerContext::default();
    let port_nine_thousand =
        server.register_service_listener::<Connection>("127.0.0.1:9000", server_context.clone())?;

    std::thread::spawn(move || server.serve().expect("server must serve"));

    tokio::spawn(port_nine_thousand)
        .await
        .expect("service must serve");
    Ok(())
}

#[derive(Default, Clone)]
struct ServerContext {
    connections: Arc<AtomicUsize>,
}

struct Connection {
    _seen: usize,
}

impl ConnectionLifecycle for Connection {
    type Deserializer = StringSerializer;
    type Serializer = StringSerializer;
    type ServerState = ServerContext;
    type MessageFuture = BoxFuture<'static, String>;

    fn on_connect(
        server_state: &Self::ServerState,
    ) -> (Self, Self::Serializer, Self::Deserializer) {
        let seen = server_state
            .connections
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        log::info!("connected {seen}");
        (Self { _seen: 0 }, StringSerializer, StringSerializer)
    }

    fn on_message(
        &mut self,
        mut message: <Self::Deserializer as Deserializer>::Request,
    ) -> Self::MessageFuture {
        // you can execute this directly or you can spawn it. Either way works -
        // it is your choice which thread you run requests on.
        // Note that you receive &mut self here: This is invoked serially per connection.
        tokio::spawn(async move {
            let seconds: u64 = message
                .split_ascii_whitespace()
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            message.push_str(" RAN");
            message
        })
        .unwrap_or_else(|_e| "join error".to_string())
        .boxed()
    }
}

struct StringSerializer;

impl Serializer for StringSerializer {
    type Response = String;

    fn encode(&mut self, mut response: Self::Response, buffer: &mut impl bytes::BufMut) {
        response.push_str(" ENCODED\n");
        buffer.put(response.as_bytes());
    }
}
impl Deserializer for StringSerializer {
    type Request = String;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Request), DeserializeError> {
        let mut read_buffer: [u8; 1] = [0; 1];
        let read = buffer
            .reader()
            .read(&mut read_buffer)
            .map_err(|_e| DeserializeError::InvalidBuffer)?;
        match String::from_utf8(read_buffer.to_vec()) {
            Ok(s) => {
                let mut s = s.trim().to_string();
                if s.is_empty() {
                    Err(DeserializeError::SkipMessage { distance: read })
                } else {
                    s.push_str(" DECODED");
                    Ok((read, s))
                }
            }
            Err(e) => {
                log::debug!("invalid message {e:?}");
                Err(DeserializeError::InvalidBuffer)
            }
        }
    }
}

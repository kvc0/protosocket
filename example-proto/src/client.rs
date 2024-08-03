use std::{
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

use messages::{EchoRequest, Request, Response};
use protosocket::{MessageReactor, ReactorStatus};

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
        .enable_all()
        .build()?;

    runtime.block_on(run_main())
}

async fn run_main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let (mut registry, registry_driver) = protosocket_prost::ClientRegistry::new()?;
    registry.set_max_message_length(512);
    let io = registry_driver.handle_io_on_dedicated_thread()?;

    let response_count = Arc::new(AtomicUsize::new(0));

    for _i in 0..1 {
        let (outbound, connection_driver) = registry
            .register_client::<Request, Response, ProtoCompletionReactor>(
                "127.0.0.1:9000",
                ProtoCompletionReactor {
                    count: response_count.clone(),
                },
            )
            .await?;
        let _connection_driver = tokio::spawn(connection_driver);
        let _client_runtime = tokio::spawn(run_message_generator(outbound));
    }

    let metrics = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let start = Instant::now();
        loop {
            interval.tick().await;
            let total = response_count.load(std::sync::atomic::Ordering::Relaxed);
            let hz = (total as f64) / start.elapsed().as_secs_f64().max(0.1);

            eprint!("\rMessages: {total:10}, rate: {hz:9.1}hz");
        }
    });

    tokio::select!(
        // _ = connection_driver => {
        //     log::warn!("connection driver quit");
        // }
        // _ = client_runtime => {
        //     log::warn!("client runtime quit");
        // }
        _ = metrics => {
            log::warn!("metrics runtime quit");
        }
    );
    io.join().expect("io completes");

    Ok(())
}

struct ProtoCompletionReactor {
    count: Arc<AtomicUsize>,
}
impl MessageReactor for ProtoCompletionReactor {
    type Inbound = Response;

    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus {
        log::debug!("received message response batch");
        for response in messages.into_iter() {
            assert_eq!(
                response.request_id,
                response
                    .body
                    .unwrap_or_default()
                    .message
                    .parse()
                    .unwrap_or_default()
            );
            assert_ne!(response.request_id, 0, "received bad message");
            self.count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        ReactorStatus::Continue
    }
}

async fn run_message_generator(outbound: tokio::sync::mpsc::Sender<Request>) {
    log::debug!("running producer");
    let mut i = 1;
    let mut timer = tokio::time::interval(Duration::from_secs(1) / 100_000);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        match outbound
            .send(Request {
                request_id: i,
                body: Some(EchoRequest {
                    message: i.to_string(),
                }),
            })
            .await
        {
            Ok(_) => {
                i += 1;
            }
            Err(e) => {
                log::error!("send should work: {e:?}");
            }
        }
    }
}

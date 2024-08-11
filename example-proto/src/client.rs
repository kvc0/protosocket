use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use messages::{EchoRequest, Request, Response};
use protosocket::{MessageReactor, ReactorStatus};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
        .enable_all()
        .build()?;

    runtime.block_on(run_main())
}

async fn run_main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let mut registry = protosocket_prost::ClientRegistry::new(tokio::runtime::Handle::current());
    registry.set_max_read_buffer_length(2 * (2 << 20));

    let response_count = Arc::new(AtomicUsize::new(0));
    let latency = Arc::new(histogram::AtomicHistogram::new(7, 52).expect("histogram works"));

    for _i in 0..2 {
        let concurrent_count = Arc::new(Semaphore::new(256));
        let concurrent = Arc::new(Mutex::new(HashMap::with_capacity(
            concurrent_count.available_permits(),
        )));
        let outbound = registry
            .register_client::<Request, Response, ProtoCompletionReactor>(
                std::env::var("ENDPOINT").unwrap_or_else(|_| "127.0.0.1:9000".to_string()),
                ProtoCompletionReactor {
                    count: response_count.clone(),
                    latency: latency.clone(),
                    concurrent: concurrent.clone(),
                    concurrent_wip: Default::default(),
                },
            )
            .await?;
        let _client_runtime = tokio::spawn(run_message_generator(
            concurrent_count,
            concurrent.clone(),
            outbound,
        ));
    }

    let metrics = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            let start = Instant::now();
            interval.tick().await;
            let total = response_count.swap(0, std::sync::atomic::Ordering::Relaxed);
            let hz = (total as f64) / start.elapsed().as_secs_f64().max(0.1);

            let latency = latency.drain();
            let p90 = latency
                .percentile(0.9)
                .unwrap_or_default()
                .map(|b| *b.range().end())
                .unwrap_or_default() as f64
                / 1000.0;
            let p999 = latency
                .percentile(0.999)
                .unwrap_or_default()
                .map(|b| *b.range().end())
                .unwrap_or_default() as f64
                / 1000.0;
            let p9999 = latency
                .percentile(0.9999)
                .unwrap_or_default()
                .map(|b| *b.range().end())
                .unwrap_or_default() as f64
                / 1000.0;
            eprintln!("Messages: {total:10} rate: {hz:9.1}hz p90: {p90:6.1}µs p999: {p999:6.1}µs p9999: {p9999:6.1}µs");
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

    Ok(())
}

struct ProtoCompletionReactor {
    count: Arc<AtomicUsize>,
    latency: Arc<histogram::AtomicHistogram>,
    concurrent: Arc<Mutex<HashMap<u64, OwnedSemaphorePermit>>>,
    concurrent_wip: HashMap<u64, OwnedSemaphorePermit>,
}
impl MessageReactor for ProtoCompletionReactor {
    type Inbound = Response;

    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus {
        log::debug!("received message response batch");
        // make sure you hold the concurrent lock as briefly as possible.
        // the permits will be released and the threads will race to lock
        // otherwise. This could also be triple-buffered so the lock is
        // only the duration of a pointer swap.
        self.concurrent_wip
            .extend(self.concurrent.lock().expect("mutex works").drain());
        for response in messages.into_iter() {
            drop(
                self.concurrent_wip
                    .remove(&response.request_id)
                    .expect("must not receive messages that have not been sent"),
            );
            let latency = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time works")
                .as_nanos() as u64
                - response.body.as_ref().expect("must have a body").nanotime;
            let _ = self.latency.increment(latency);
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

async fn run_message_generator(
    concurrent_count: Arc<Semaphore>,
    concurrent: Arc<Mutex<HashMap<u64, OwnedSemaphorePermit>>>,
    outbound: tokio::sync::mpsc::Sender<Request>,
) {
    log::debug!("running producer");
    let mut i = 1;
    loop {
        let permit = concurrent_count
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore works");
        concurrent.lock().expect("mutex works").insert(i, permit);
        match outbound
            .send(Request {
                request_id: i,
                body: Some(EchoRequest {
                    message: i.to_string(),
                    nanotime: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("time works")
                        .as_nanos() as u64,
                }),
            })
            .await
        {
            Ok(_) => {
                i += 1;
            }
            Err(e) => {
                log::error!("send should work: {e:?}");
                return;
            }
        }
    }
}

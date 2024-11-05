use std::{
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use messages::{EchoRequest, Request, Response};
use protosocket_rpc_client::RpcClient;
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
        .enable_all()
        .build()?;

    runtime.block_on(run_main())
}

async fn run_main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let response_count = Arc::new(AtomicUsize::new(0));
    let latency = Arc::new(histogram::AtomicHistogram::new(7, 52).expect("histogram works"));

    for _i in 0..2 {
        let concurrent_count = Arc::new(Semaphore::new(256));

        let (client, connection) = protosocket_rpc_client::connect::<
            protosocket_prost::ProstSerializer<Response, Request>,
            protosocket_prost::ProstSerializer<Response, Request>,
        >(
            std::env::var("ENDPOINT")
                .unwrap_or_else(|_| "127.0.0.1:9000".to_string())
                .parse()
                .expect("must use a valid socket address"),
            &Default::default(),
        )
        .await?;
        let _connection_handle = tokio::spawn(connection);
        let _client_handle = tokio::spawn(generate_traffic(
            concurrent_count,
            client,
            response_count.clone(),
            latency.clone(),
        ));
    }

    let metrics = tokio::spawn(print_periodic_metrics(response_count, latency));

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

async fn print_periodic_metrics(
    response_count: Arc<AtomicUsize>,
    latency: Arc<histogram::AtomicHistogram>,
) {
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
}

async fn generate_traffic(
    concurrent_count: Arc<Semaphore>,
    client: RpcClient<Request, Response>,
    metrics_count: Arc<AtomicUsize>,
    metrics_latency: Arc<histogram::AtomicHistogram>,
) {
    log::debug!("running traffic generator");
    let mut i = 1;
    loop {
        let permit = concurrent_count
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore works");
        match client
            .send_unary(Request {
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
            Ok(completion) => {
                i += 1;
                let metrics_count = metrics_count.clone();
                let metrics_latency = metrics_latency.clone();
                tokio::spawn(async move {
                    let response = completion.await.expect("response must be successful");
                    handle_response(response, metrics_count, metrics_latency);
                    drop(permit);
                });
            }
            Err(e) => {
                log::error!("send should work: {e:?}");
                return;
            }
        }
    }
}

fn handle_response(
    response: Response,
    metrics_count: Arc<AtomicUsize>,
    metrics_latency: Arc<histogram::AtomicHistogram>,
) {
    let latency = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time works")
        .as_nanos() as u64
        - response.body.as_ref().expect("must have a body").nanotime;
    let _ = metrics_latency.increment(latency);
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
    metrics_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

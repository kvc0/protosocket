use std::{
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures::{stream::FuturesUnordered, task::SpawnExt, StreamExt};
use messages::{EchoRequest, EchoResponseKind, Request, Response, ResponseBehavior};
use protosocket::PooledEncoder;
use protosocket_prost::{ProstDecoder, ProstSerializer};
use protosocket_rpc::{
    client::{Configuration, RpcClient, UnverifiedTlsStreamConnector},
    ProtosocketControlCode,
};
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
        .event_interval(7)
        .enable_all()
        .build()?;

    runtime.block_on(run_main())
}

async fn run_main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let response_count = Arc::new(AtomicUsize::new(0));
    let latency = Arc::new(histogram::AtomicHistogram::new(7, 52).expect("histogram works"));

    let max_concurrent = 32;
    let concurrent_count = Arc::new(AtomicUsize::new(0));

    for _i in 0..8 {
        let (client, connection) = protosocket_rpc::client::connect::<
            PooledEncoder<ProstSerializer<Request>>,
            ProstDecoder<Response>,
            _,
        >(
            std::env::var("ENDPOINT")
                .unwrap_or_else(|_| "127.0.0.1:9000".to_string())
                .parse()
                .expect("must use a valid socket address"),
            &Configuration::new(UnverifiedTlsStreamConnector::new(
                "localhost".try_into().expect("must be a valid server name"),
            )),
        )
        .await?;
        let _connection_handle = tokio::spawn(connection);
        let concurrency_limit = Arc::new(Semaphore::new(max_concurrent));
        let _client_handle = tokio::spawn(generate_traffic(
            concurrent_count.clone(),
            concurrency_limit,
            client,
            response_count.clone(),
            latency.clone(),
        ));
    }

    let metrics = tokio::spawn(print_periodic_metrics(
        response_count,
        latency,
        concurrent_count,
    ));

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
    concurrent_count: Arc<AtomicUsize>,
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
        let concurrent = concurrent_count.load(std::sync::atomic::Ordering::Relaxed);
        eprintln!("Messages: {total:10} rate: {hz:9.1}hz p90: {p90:6.1}µs p999: {p999:6.1}µs p9999: {p9999:6.1}µs concurrency: {concurrent}");
    }
}

async fn generate_traffic(
    concurrent_count: Arc<AtomicUsize>,
    concurrency_limit: Arc<Semaphore>,
    client: RpcClient<Request, Response>,
    metrics_count: Arc<AtomicUsize>,
    metrics_latency: Arc<histogram::AtomicHistogram>,
) {
    log::debug!("running traffic generator");
    let mut i = 1;
    let mut wip = FuturesUnordered::new();
    loop {
        let permit = tokio::select! {
            permit = concurrency_limit
            .clone()
            .acquire_owned() => {
                permit.expect("semaphore works")
            }
            _ = wip.select_next_some() => {
                // completed one
                continue
            }
        };

        if true {
            match client
                .send_unary(Request {
                    request_id: i,
                    code: ProtosocketControlCode::Normal as u32,
                    body: Some(EchoRequest {
                        message: i.to_string(),
                        nanotime: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .expect("time works")
                            .as_nanos() as u64,
                    }),
                    response_behavior: ResponseBehavior::Unary as i32,
                })
                .await
            {
                Ok(completion) => {
                    concurrent_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    i += 1;
                    let metrics_count = metrics_count.clone();
                    let metrics_latency = metrics_latency.clone();
                    let concurrent_count: Arc<AtomicUsize> = concurrent_count.clone();
                    wip.spawn(async move {
                        let response = completion.await.expect("response must be successful");
                        handle_response(response, metrics_count, metrics_latency);
                        concurrent_count.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        drop(permit);
                    })
                    .expect("can spawn");
                }
                Err(e) => {
                    log::error!("send should work: {e:?}");
                    return;
                }
            }
        } else {
            // fixme: the streaming math is wrong
            match client
                .send_streaming(Request {
                    request_id: i,
                    code: ProtosocketControlCode::Normal as u32,
                    body: Some(EchoRequest {
                        message: i.to_string(),
                        nanotime: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .expect("time works")
                            .as_nanos() as u64,
                    }),
                    response_behavior: ResponseBehavior::Stream as i32,
                })
                .await
            {
                Ok(mut completion) => {
                    i += 1;
                    let metrics_count = metrics_count.clone();
                    let metrics_latency = metrics_latency.clone();
                    wip.spawn(async move {
                        while let Some(Ok(response)) = completion.next().await {
                            handle_stream_response(
                                response,
                                metrics_count.clone(),
                                metrics_latency.clone(),
                            );
                        }
                        drop(permit);
                    })
                    .expect("can spawn");
                }
                Err(e) => {
                    log::error!("send should work: {e:?}");
                    return;
                }
            }
        }
    }
}

fn handle_response(
    response: Response,
    metrics_count: Arc<AtomicUsize>,
    metrics_latency: Arc<histogram::AtomicHistogram>,
) {
    metrics_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let request_id = response.request_id;
    assert_ne!(response.request_id, 0, "received bad message");
    match response.kind {
        Some(EchoResponseKind::Echo(echo)) => {
            assert_eq!(request_id, echo.message.parse().unwrap_or_default());

            let latency = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time works")
                .as_nanos() as u64
                - echo.nanotime;
            let _ = metrics_latency.increment(latency);
        }
        Some(EchoResponseKind::Stream(_char_response)) => {
            log::error!("got a stream response for a unary request");
        }
        None => {
            log::warn!("no response body");
        }
    }
}

fn handle_stream_response(
    response: Response,
    metrics_count: Arc<AtomicUsize>,
    metrics_latency: Arc<histogram::AtomicHistogram>,
) {
    log::debug!("received stream response {response:?}");
    metrics_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let request_id = response.request_id;
    assert_ne!(response.request_id, 0, "received bad message");
    match response.kind {
        Some(EchoResponseKind::Echo(_echo)) => {
            log::error!("got a unary response for a stream request");
        }
        Some(EchoResponseKind::Stream(char_response)) => {
            let places = (request_id as f64).log10().ceil() as u32;
            let place = places - char_response.sequence as u32;
            let column = (request_id / 10u64.pow(place)) % 10;

            assert_eq!(Ok(column), char_response.message.parse());

            if place == places - 1 {
                let latency = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time works")
                    .as_nanos() as u64
                    - char_response.nanotime;
                let _ = metrics_latency.increment(latency);
            }
        }
        None => {
            log::warn!("no response body");
        }
    }
}

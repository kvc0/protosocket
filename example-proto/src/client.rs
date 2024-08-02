use std::{
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};

use messages::{EchoRequest, Request, Response};

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
        let (outbound, inbound, connection_driver) = registry
            .register_client::<Request, Response>("127.0.0.1:9000")
            .await?;
        let count = response_count.clone();
        let _connection_driver = tokio::spawn(connection_driver);
        let _client_runtime = tokio::spawn(run_application(outbound, inbound, count));
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

async fn run_application(
    outbound: tokio::sync::mpsc::Sender<Request>,
    mut inbound: tokio::sync::mpsc::Receiver<Response>,
    count: Arc<AtomicUsize>,
) {
    let producer = async move {
        log::debug!("running producer");
        let mut i = 1;
        loop {
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
    };

    let consumer = async move {
        log::debug!("running consumer");
        let mut receive_buffer = Vec::new();
        loop {
            let received = inbound.recv_many(&mut receive_buffer, 16).await;
            if received == 0 {
                log::warn!("inbound message channel was closed");
                return;
            }
            for response in receive_buffer.drain(..) {
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
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    };
    tokio::select! {
        _ = tokio::spawn(producer) => {
            log::error!("producer ended")
        }
        _ = tokio::spawn(consumer) => {
            log::error!("consumer ended")
        }
    }
}

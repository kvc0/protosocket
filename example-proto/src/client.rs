use std::{sync::{atomic::AtomicUsize, Arc}, time::{Duration, Instant}};

use futures::stream::FuturesUnordered;
use messages::{EchoRequest, Request, Response};

mod messages;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let (registry, driver) = protosocket_prost::ClientRegistry::new()?;
    let registry_driver = tokio::spawn(driver);

    let (outbound, mut inbound, connection_driver) = registry.register_client::<Request, Response>("127.0.0.1:9000").await?;
    let connection_driver = tokio::spawn(connection_driver);

    let response_count = Arc::new(AtomicUsize::new(0));
    
    let count = response_count.clone();
    let client_runtime = tokio::spawn(async move {
        let mut i = 0;
        let mut j = 0;
        loop {
            if i - j < 100 {
                match outbound.send(Request { request_id: i, body: Some(EchoRequest { message: i.to_string() }) }).await {
                    Ok(_) => {
                        i += 1;
                    }
                    Err(e) => {
                        log::error!("send should work: {e:?}");
                    }
                }
            }
            if let Ok(response) = inbound.try_recv() {
                j += 1;
                assert_eq!(response.request_id, response.body.unwrap_or_default().message.parse().unwrap_or_default());
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            tokio::task::yield_now().await;
        }
    });

    let metrics = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let start = Instant::now();
        loop {
            interval.tick().await;
            let total = response_count.load(std::sync::atomic::Ordering::Relaxed);
            let hz = (total as f64) / start.elapsed().as_secs_f64().max(0.1);

            print!("\rMessages: {total:10}, rate: {hz:.1}hz");
        }
    });

    tokio::select!(
        _ = registry_driver => {
            log::warn!("registry driver quit");
        }
        _ = connection_driver => {
            log::warn!("connection driver quit");
        }
        _ = client_runtime => {
            log::warn!("client runtime quit");
        }
        _ = metrics => {
            log::warn!("metrics runtime quit");
        }
    );

    Ok(())
}

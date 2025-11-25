use std::{sync::atomic::AtomicUsize, time::Instant};

use criterion::{criterion_main, Criterion};
use tokio::sync::mpsc;

fn mpsc_or_queues(criterion: &mut Criterion) {
    env_logger::builder().is_test(true).try_init().unwrap();
    let mut group = criterion.benchmark_group("mpsc_or_spillway");
    group.throughput(criterion::Throughput::Elements(1));
    let threads = 16;

    group.bench_function("mpsc", |bencher| {
        bencher
            .to_async(runtime(threads))
            .iter_custom(async |size| {
                let (send, mut receive) = mpsc::channel(128);
                let receiver = tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut i = 0;
                    while receive.recv_many(&mut buffer, 128).await != 0 {
                        for v in buffer.drain(..) {
                            i += v;
                        }
                    }
                    i
                });

                for _ in 0..threads {
                    let send = send.clone();
                    tokio::spawn(async move {
                        for _ in 0..(size as usize / threads).max(1) {
                            let _ = send.send(1_usize).await;
                        }
                    });
                }
                drop(send);

                let start = Instant::now();
                let n = receiver.await;
                let elapsed = start.elapsed();
                assert_eq!(
                    n.expect("must join successfully"),
                    (size as usize / threads).max(1) * threads
                );
                elapsed
            });
    });

    group.bench_function("spillway", |bencher| {
        bencher
            .to_async(runtime(threads))
            .iter_custom(async |size| {
                let (send, mut receive) = spillway::channel(threads);
                let receiver = tokio::spawn(async move {
                    let mut i = 0;
                    while let Some(v) = receive.next().await {
                        i += v;
                    }
                    i
                });

                for _ in 0..threads {
                    let send = send.clone();
                    tokio::spawn(async move {
                        for _ in 0..(size as usize / threads).max(1) {
                            let _ = send.send(1_usize);
                        }
                    });
                }
                drop(send);

                let start = Instant::now();
                let n = receiver.await;
                let elapsed = start.elapsed();
                log::info!("ok {n:?}");
                assert_eq!(
                    n.expect("must join successfully"),
                    (size as usize / threads).max(1) * threads
                );
                elapsed
            });
    });
}

fn runtime(threads: usize) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .thread_name_fn(|| {
            static I: AtomicUsize = AtomicUsize::new(0);
            format!(
                "worker-{}",
                I.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        })
        .build()
        .expect("can build tokio runtime")
}

criterion_main! {
    stack,
}
criterion::criterion_group!(stack, mpsc_or_queues);

use std::{collections::VecDeque, hint::black_box, sync::{Arc, Mutex, atomic::AtomicUsize}, time::Instant};

use criterion::{criterion_main, Criterion};
use futures::task::AtomicWaker;
use tokio::sync::mpsc;

fn mpsc_or_queues(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("mpsc_or_queues");
    let threads = 16;

    group.bench_function("mpsc", |bencher| {
        bencher.to_async(runtime(threads)).iter_custom(async |size| {
            let (send, mut receive) = mpsc::channel(128);
            let receiver = tokio::spawn(async move {
                let mut buffer = Vec::new();
                while receive.recv_many(&mut buffer, 128).await != 0 {
                    for v in buffer.drain(..) {
                        black_box(v);
                    }
                }
            });

            for _ in 0..threads {
                let send = send.clone();
                tokio::spawn(async move {
                    for _ in 0..(size as usize/threads).max(1) {
                        let _ = send.send("a value").await;
                    }
                });
            }
            drop(send);

            let start = Instant::now();
            let _ = receiver.await;
            start.elapsed()
        });
    });

    group.bench_function("queues", |bencher| {
        bencher.to_async(runtime(threads)).iter_custom(async |size| {
            let (send, mut receive) = channel(threads);
            let receiver = tokio::spawn(async move {
                while let Some(v) = receive.next().await {
                    black_box(v);
                }
            });

            for _ in 0..threads {
                let send = send.clone();
                tokio::spawn(async move {
                    for _ in 0..(size as usize/threads).max(1) {
                        let _ = send.send("a value");
                    }
                });
            }
            drop(send);

            let start = Instant::now();
            let _ = receiver.await;
            start.elapsed()
        });
    });
}

fn runtime(threads: usize) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .thread_name_fn(|| {
            static I: AtomicUsize = AtomicUsize::new(0);
            format!("worker-{}", I.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
        })
        .build()
        .expect("can build tokio runtime")
}

pub fn channel<T>(concurrency: usize) -> (Sender<T>, Receiver<T>) {
    let sender = Sender::<T> {
        channels: (0..concurrency).into_iter().map(|_| Default::default()).collect::<Vec<_>>().into(),
        slot: rand::random_range(0..concurrency),
        waker: AtomicWaker::new().into(),
        senders: AtomicUsize::new(1).into(),
    };
    let receiver = Receiver {
        channels: sender.channels.clone(),
        waker: sender.waker.clone(),
        senders: sender.senders.clone(),
        cursor: 0,
        buffer: Default::default(),
    };
    (sender, receiver)
}

pub struct Sender<T> {
    channels: Arc<Vec<Mutex<VecDeque<T>>>>,
    slot: usize,
    waker: Arc<futures::task::AtomicWaker>,
    senders: Arc<AtomicUsize>,
}
impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.senders.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { channels: self.channels.clone(), slot: rand::random_range(0..self.channels.len()), waker: self.waker.clone(), senders: self.senders.clone() }
    }
}
impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.senders.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        self.waker.wake();
    }
}

impl<T> Sender<T> {
    pub fn send(&self, value: T) {
        let mut channel = self.channels[self.slot].lock().expect("must not be poisoned");
        channel.push_back(value);
        if channel.len() == 1 {
            self.waker.wake();
        }
    }
}

pub struct Receiver<T> {
    channels: Arc<Vec<Mutex<VecDeque<T>>>>,
    cursor: usize,
    buffer: VecDeque<T>,
    waker: Arc<futures::task::AtomicWaker>,
    senders: Arc<AtomicUsize>,
}
impl<T> Receiver<T> {
    pub fn poll_next(&mut self, context: &mut std::task::Context) -> std::task::Poll<Option<T>> {
        let next = if self.buffer.is_empty() {
            let wraparound = self.cursor;
            self.cursor = (self.cursor + 1) % self.channels.len();
            while self.cursor != wraparound {
                std::mem::swap(&mut *self.channels[self.cursor].lock().expect("must not be poisoned"), &mut self.buffer);
                self.cursor = (self.cursor + 1) % self.channels.len();
                if !self.buffer.is_empty() {
                    break
                }
            }
            if self.buffer.is_empty() {
                self.waker.register(context.waker());
                // gotta do a double-check or else waker can drop liveness events due to toctou
                if !self.channels.iter().all(|channel| channel.lock().expect("must not be poisoned").is_empty()) {
                    self.waker.wake();
                }
                if self.senders.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                    return std::task::Poll::Ready(None);
                }
                return std::task::Poll::Pending
            }
            self.buffer.pop_front().expect("already checked")
        } else {
            self.buffer.pop_front().expect("already checked")
        };

        std::task::Poll::Ready(Some(next))
    }

    #[inline]
    pub async fn next(&mut self) -> Option<T> {
        std::future::poll_fn(|context| self.poll_next(context)).await
    }
}


criterion_main! {
    stack,
}
criterion::criterion_group!(stack, mpsc_or_queues);

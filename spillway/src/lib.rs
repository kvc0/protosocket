#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod receiver;
mod sender;
mod shared;

use std::sync::Arc;

pub use receiver::Receiver;
pub use sender::Sender;

/// Get a new spillway channel with a default concurrency level.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    // const PARALLELISM: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    //     std::thread::available_parallelism()
    //         .map(std::num::NonZero::get)
    //         .unwrap_or(2)
    //         .max(2)
    // });
    channel_with_concurrency(8)
}

/// Get a new spillway channel with the given concurrency level.
///
/// Use this when you need lots of parallelism, or when you know how many Senders
/// you will have. Higher numbers reduce contention, but increase the cost of
/// parking the Receiver when idle. Thread count is a good starting point for
/// concurrency.
pub fn channel_with_concurrency<T>(concurrency: usize) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(shared::Shared::new(concurrency));
    let sender = Sender::new(shared.clone());
    let receiver = Receiver::new(shared);

    (sender, receiver)
}

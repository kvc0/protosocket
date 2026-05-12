#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

mod error;
mod receiver;
mod sender;
mod shared;

use std::sync::Arc;

pub use error::Error;
pub use receiver::Receiver;
pub use sender::Sender;

/// Get a new spillway channel with a default concurrency level and no capacity limit.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    // const PARALLELISM: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    //     std::thread::available_parallelism()
    //         .map(std::num::NonZero::get)
    //         .unwrap_or(2)
    //         .max(2)
    // });
    channel_with_concurrency(8)
}

/// Get a new spillway channel with the given concurrency level and no capacity limit.
///
/// Use this when you need lots of parallelism, or when you know how many Senders
/// you will have. Higher numbers reduce contention, but increase the cost of
/// parking the Receiver when idle. Thread count is a good starting point for
/// concurrency.
pub fn channel_with_concurrency<T>(concurrency: usize) -> (Sender<T>, Receiver<T>) {
    channel_with_capacity_and_concurrency(u64::MAX, concurrency)
}

/// Get a new spillway channel with a soft capacity limit and the given concurrency level.
///
/// `capacity` is an upper bound on the number of in-flight values. Sends are
/// rejected with [`Error::Full`] when the channel is at or above this limit.
///
/// Mind your batch sizes when using a capacity limit. If you have a capacity of 10 and you
/// send 11 values in a batch, the entire batch will be rejected.
///
/// Pass `u64::MAX` to disable the limit (this matches [`channel_with_concurrency`]).
pub fn channel_with_capacity_and_concurrency<T>(
    capacity: u64,
    concurrency: usize,
) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(shared::Shared::new(concurrency, capacity));
    let sender = Sender::new(shared.clone());
    let receiver = Receiver::new(shared);

    (sender, receiver)
}

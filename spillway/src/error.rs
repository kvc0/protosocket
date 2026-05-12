/// Errors returned by Spillway senders.
///
/// The unsent value(s) are returned in the error variant so the caller can
/// reuse or drop them as appropriate.
#[derive(Debug, thiserror::Error)]
pub enum Error<T> {
    /// The channel is at or above its soft capacity. Nothing was enqueued.
    #[error("spillway channel is full")]
    Full(T),
    /// The Receiver has been dropped. The channel will never accept more values.
    #[error("spillway channel is closed")]
    Closed(T),
}

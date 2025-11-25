mod receiver;
mod sender;
mod shared;

use std::sync::Arc;

pub use receiver::Receiver;
pub use sender::Sender;

pub fn channel<T>(concurrency: usize) -> (Sender<T>, Receiver<T>) {
    let shared = Arc::new(shared::Shared::new(concurrency));
    let sender = Sender::new(shared.clone());
    let receiver = Receiver::new(shared);

    (sender, receiver)
}

use std::{collections::VecDeque, sync::{Mutex, atomic::AtomicUsize}};

pub struct Shared<T> {
    pub(crate) chutes: Vec<Mutex<VecDeque<T>>>,
    pub(crate) waker: futures::task::AtomicWaker,
    pub(crate) senders: AtomicUsize,
}

impl<T> Shared<T> {
    pub fn new(concurrency: usize) -> Self {
        Self {
            chutes: (0..concurrency).into_iter().map(|_| Default::default()).collect::<Vec<_>>().into(),
            waker: futures::task::AtomicWaker::new().into(),
            senders: AtomicUsize::new(0).into(),
        }
    }

    pub fn add_sender(&self) {
        self.senders.fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    
    pub fn drop_sender(&self) -> usize {
        self.senders.fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
    }

    pub fn choose_chute(&self) -> usize {
        rand::random_range(0..self.chutes.len())
    }

    pub fn wake(&self) {
        self.waker.wake();
    }

    pub fn send(&self, chute: usize, value: T) {
        let mut chute = self.chutes[chute].lock().expect("must not be poisoned");
        chute.push_back(value);
        if chute.len() == 1 {
            // only bother with waking when we transition to a non-empty state.
            // the receiver task will be pending for other reasons as long as there are entries in at least one chute.
            self.waker.wake();
        }
    }
}

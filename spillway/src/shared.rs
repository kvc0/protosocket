use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        Mutex,
    },
};

pub struct Shared<T> {
    pub(crate) chutes: Vec<Mutex<VecDeque<T>>>,
    pub(crate) waker: futures::task::AtomicWaker,
    pub(crate) senders: AtomicUsize,
    dead: AtomicBool,
}

impl<T> Shared<T> {
    pub fn new(concurrency: usize) -> Self {
        Self {
            chutes: (0..concurrency)
                .map(|_| Default::default())
                .collect::<Vec<_>>(),
            waker: futures::task::AtomicWaker::new(),
            senders: AtomicUsize::new(0),
            dead: AtomicBool::new(false),
        }
    }

    pub fn add_sender(&self) {
        self.senders
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    pub fn drop_sender(&self) -> usize {
        self.senders
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
    }

    pub fn choose_chute(&self) -> usize {
        rand::random_range(0..self.chutes.len())
    }

    pub fn wake(&self) {
        self.waker.wake();
    }

    pub fn send(&self, chute: usize, value: T) -> Result<(), T> {
        if self.dead.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(value);
        }
        let mut chute = self.chutes[chute].lock().expect("must not be poisoned");
        chute.push_back(value);
        if chute.len() == 1 {
            // only bother with waking when we transition to a non-empty state.
            // the receiver task will be pending for other reasons as long as there are entries in at least one chute.
            self.waker.wake();
        }
        Ok(())
    }
}

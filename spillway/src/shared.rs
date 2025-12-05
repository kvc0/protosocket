use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize},
        Mutex,
    },
};

pub struct Chute<T> {
    queue: Mutex<VecDeque<T>>,
    clean: AtomicBool,
}
impl<T> std::fmt::Debug for Chute<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chute")
            .field("queue", &self.queue.lock().expect("not poisoned").len())
            .field("clean", &self.clean)
            .finish()
    }
}
impl<T> Default for Chute<T> {
    fn default() -> Self {
        Self {
            queue: Default::default(),
            clean: AtomicBool::new(true),
        }
    }
}
impl<T> Chute<T> {
    pub fn send(&self, t: T, on_dirty: impl FnOnce()) {
        let mut queue = self.queue.lock().expect("must not be poisoned");
        let length = queue.len();
        queue.push_back(t);
        if length == 0 {
            self.clean.swap(false, std::sync::atomic::Ordering::AcqRel);
            on_dirty()
        } else {
            log::debug!("sent one, length: {}", length + 1);
        }
    }

    pub fn clean(&self) -> bool {
        self.clean.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn swap(&self, new_queue: &mut VecDeque<T>) {
        debug_assert_eq!(
            0,
            new_queue.len(),
            "you can't swap in a queue with contents!"
        );
        let mut queue = self.queue.lock().expect("must not be poisoned");
        std::mem::swap(&mut *queue, new_queue);
        self.clean.store(true, std::sync::atomic::Ordering::Release);
    }
}

pub struct Shared<T> {
    pub(crate) chutes: Vec<Chute<T>>,
    pub(crate) waker: futures::task::AtomicWaker,
    pub(crate) senders: AtomicUsize,
    pub(crate) chute_clock: AtomicUsize,
    dead: AtomicBool,
}

impl<T> std::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("chutes", &self.chutes)
            .field("waker", &self.waker)
            .field("senders", &self.senders)
            .field("chute_clock", &self.chute_clock)
            .field("dead", &self.dead)
            .finish()
    }
}

impl<T> Shared<T> {
    pub fn new(concurrency: usize) -> Self {
        Self {
            chutes: (0..concurrency)
                .map(|_| Default::default())
                .collect::<Vec<_>>(),
            waker: futures::task::AtomicWaker::new(),
            senders: AtomicUsize::new(0),
            chute_clock: AtomicUsize::new(0),
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
        self.chute_clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.chutes.len()
    }

    pub fn wake(&self) {
        self.waker.wake();
    }

    pub fn send(&self, chute: usize, value: T) -> Result<(), T> {
        if self.dead.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(value);
        }
        self.chutes[chute].send(value, || {
            // only bother with waking when we transition to a non-empty state.
            // the receiver task will be pending for other reasons as long as there are entries in at least one chute.
            self.waker.wake();
            log::debug!("woke for clean falling edge: {chute}");
        });
        log::debug!("sent one: {chute}");
        Ok(())
    }

    pub fn race_find_dirty(&self, starting_wrap_offset: usize) -> Option<usize> {
        for i in 0..self.chutes.len() {
            if !self.chutes[(starting_wrap_offset + i) % self.chutes.len()].clean() {
                return Some((starting_wrap_offset + i) % self.chutes.len());
            }
        }

        None
    }
}

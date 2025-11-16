use std::sync::atomic::AtomicUsize;

use crate::{runtime::LevelRuntime, worker::LevelWorker};

/// A setup builder for your level runtime.
///
/// See `tokio::runtime::Builder` for more detailed documentation.
/// A level runtime is a collection of tokio current-thread runtimes.
pub struct Builder {
    builder: tokio::runtime::Builder,
    worker_threads: usize,
    thread_name: std::sync::Arc<dyn Fn() -> String + Send + Sync + 'static>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            worker_threads: 1,
            builder: tokio::runtime::Builder::new_current_thread(),
            thread_name: std::sync::Arc::new(|| {
                static I: AtomicUsize = AtomicUsize::new(0);
                let i = I.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                format!("level-{i:02}")
            }),
        }
    }
}

impl Builder {
    /// Build a new level runtime
    pub fn build(&mut self) -> LevelRuntime {
        LevelRuntime::from_workers(
            self.thread_name.clone(),
            (0..self.worker_threads)
                .map(|_i| {
                    self.builder
                        .build()
                        .expect("must be able to build tokio runtime")
                })
                .map(LevelWorker::from_runtime)
                .collect(),
        )
    }

    /// How many thread-local worker threads do you want?
    pub fn worker_threads(&mut self, threads: usize) -> &mut Self {
        self.worker_threads = threads;
        self
    }

    /// Set a thread name prefix for level threads.
    pub fn thread_name_prefix(&mut self, name: impl Into<String>) -> &mut Self {
        let mut name = name.into();
        if 12 < name.len() {
            name.truncate(12);
            // htop and other unix tools can see the indices. They traditionally
            // can only handle 16 byte C strings. The 16th byte is \0, so 15
            // characters is the limit. -01 takes 3 characters, so you get 12 to
            // name your prefix. (if you are using 100+ threads, please feel free
            // to pr)
            log::warn!("truncating thread name prefix to 12 characters: {name}")
        }
        let name_i: AtomicUsize = AtomicUsize::new(0);
        self.thread_name = std::sync::Arc::new(move || {
            let i = name_i.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            format!("{name}-{i:02}")
        });
        self
    }

    /// Do you want to use IO and time?
    pub fn enable_all(&mut self) -> &mut Self {
        self.builder.enable_all();
        self
    }

    /// How frequently should you epoll? (Default is 61)
    pub fn event_interval(&mut self, interval: u32) -> &mut Self {
        self.builder.event_interval(interval);
        self
    }

    /// How frequently should you check for off-runtime task submission? (Default is 31)
    pub fn global_queue_interval(&mut self, interval: u32) -> &mut Self {
        self.builder.global_queue_interval(interval);
        self
    }

    /// How many file descriptors should you epoll at a time? (Default is 1024)
    pub fn max_io_events_per_tick(&mut self, capacity: usize) -> &mut Self {
        self.builder.max_io_events_per_tick(capacity);
        self
    }

    /// How many blocking threads should each local runtime be able to create? (Devault is 512)
    pub fn max_blocking_threads(&mut self, capacity: usize) -> &mut Self {
        self.builder.max_blocking_threads(capacity);
        self
    }

    /// Notification of thread park
    pub fn on_thread_park(&mut self, f: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.builder.on_thread_park(f);
        self
    }

    /// Notification of thread unpark
    pub fn on_thread_unpark(&mut self, f: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.builder.on_thread_unpark(f);
        self
    }
}

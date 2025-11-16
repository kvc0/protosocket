use crate::{runtime::LevelRuntime, worker::LevelWorker};

pub struct Builder {
    builder: tokio::runtime::Builder,
    worker_threads: usize,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            worker_threads: 1,
            builder: tokio::runtime::Builder::new_current_thread(),
        }
    }

    pub fn build(&mut self) -> LevelRuntime {
        LevelRuntime::from_workers(
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

    pub fn worker_threads(&mut self, threads: usize) -> &mut Self {
        self.worker_threads = threads;
        self
    }

    pub fn thread_name_fn(&mut self, f: impl Fn() -> String + Send + Sync + 'static) -> &mut Self {
        self.builder.thread_name_fn(f);
        self
    }

    pub fn enable_all(&mut self) -> &mut Self {
        self.builder.enable_all();
        self
    }

    pub fn event_interval(&mut self, interval: u32) -> &mut Self {
        self.builder.event_interval(interval);
        self
    }

    pub fn global_queue_interval(&mut self, interval: u32) -> &mut Self {
        self.builder.global_queue_interval(interval);
        self
    }

    pub fn max_io_events_per_tick(&mut self, capacity: usize) -> &mut Self {
        self.builder.max_io_events_per_tick(capacity);
        self
    }

    pub fn max_blocking_threads(&mut self, capacity: usize) -> &mut Self {
        self.builder.max_blocking_threads(capacity);
        self
    }

    pub fn on_thread_park(&mut self, f: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.builder.on_thread_park(f);
        self
    }

    pub fn on_thread_unpark(&mut self, f: impl Fn() + Send + Sync + 'static) -> &mut Self {
        self.builder.on_thread_unpark(f);
        self
    }
}

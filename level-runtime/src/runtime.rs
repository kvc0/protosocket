use std::{future::Future, sync::OnceLock};

use rand::Rng;
use tokio::task::JoinHandle;

use crate::worker::{LevelWorker, LevelWorkerHandle};

static GLOBAL_RUNTIME: OnceLock<LevelRuntimeHandle> = OnceLock::new();

/// Spawn a task on one of the local runtimes, with a work balance heuristic.
///
/// Use this when you can do your work on any runtime and it does not matter which.
/// Be mindful of cross-runtime synchronization and await, which is more costly in
/// a level runtime than a regular tokio multi-thread runtime.
#[track_caller]
pub fn spawn_balanced<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match GLOBAL_RUNTIME
        .get()
        .map(|worker| worker.spawn_balanced(future))
    {
        Some(handle) => handle,
        None => {
            panic!("spawn_balanced can only be called in processes running a default level worker")
        }
    }
}

/// Spawn a copy of a task on each local runtime.
///
/// Use this to place a server listener on each of your thread local threads.
#[track_caller]
pub fn spawn_on_each<F>(future: impl Fn() -> F) -> Vec<JoinHandle<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match GLOBAL_RUNTIME
        .get()
        .map(|worker| worker.spawn_on_each(future))
    {
        Some(handle) => handle,
        None => {
            panic!("spawn_balanced can only be called in processes running a default level worker")
        }
    }
}

/// A wrapper for a collection of tokio current-thread runtimes.
///
/// It offers a load leveling heuristic, but not work stealing.
pub struct LevelRuntime {
    workers: Vec<LevelWorker>,
    thread_name: std::sync::Arc<dyn Fn() -> String + Send + Sync + 'static>,
}
impl std::fmt::Debug for LevelRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LevelRuntime")
            .field("workers", &self.workers)
            .finish()
    }
}
impl LevelRuntime {
    pub(crate) fn from_workers(
        thread_name: std::sync::Arc<dyn Fn() -> String + Send + Sync + 'static>,
        workers: Vec<LevelWorker>,
    ) -> Self {
        Self {
            workers,
            thread_name,
        }
    }

    /// Register this LevelRuntime as the default runtime for the static
    /// `level_runtime::spawn*` family functions.
    pub fn set_default(&self) {
        if GLOBAL_RUNTIME.set(self.handle()).is_err() {
            panic!("must only set one default level runtime per process")
        }
    }

    /// Get a spawn handle for the runtime.
    pub fn handle(&self) -> LevelRuntimeHandle {
        LevelRuntimeHandle {
            workers: self.workers.iter().map(LevelWorker::handle).collect(),
        }
    }

    /// Execute this runtime.
    pub fn run(self) {
        self.run_with_termination(std::future::pending());
    }

    /// Start this runtime in the background.
    pub fn start(self) {
        let _handles: Vec<_> = self
            .workers
            .into_iter()
            .map(|worker| {
                std::thread::Builder::new()
                    .name((self.thread_name)())
                    .spawn(move || {
                        worker.run(std::future::pending());
                    })
                    .expect("must be able to spawn level worker")
            })
            .collect();
    }

    /// Execute this runtime. When `termination` completes, the backing executors will close.
    pub fn run_with_termination(self, termination: impl Future<Output = ()> + Send + 'static) {
        let termination = futures::FutureExt::shared(termination);
        let handles: Vec<_> = self
            .workers
            .into_iter()
            .map(|worker| {
                let termination = termination.clone();
                std::thread::Builder::new()
                    .name((self.thread_name)())
                    .spawn(move || {
                        worker.run(termination);
                    })
                    .expect("must be able to spawn level worker")
            })
            .collect();
        for handle in handles {
            let _ = handle.join();
        }
    }
}

/// A handle to a LevelRuntime for spawning tasks.
#[derive(Clone, Debug)]
pub struct LevelRuntimeHandle {
    workers: Vec<LevelWorkerHandle>,
}

impl LevelRuntimeHandle {
    /// Spawn this future on one of the workers; choose which one based
    /// on a load heuristic.
    #[track_caller]
    pub fn spawn_balanced<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        balance_workers(&self.workers).spawn_local(future)
    }

    /// Get one consistent worker handle
    #[track_caller]
    pub fn pick_worker(&self) -> &LevelWorkerHandle {
        balance_workers(&self.workers)
    }

    /// Spawn a copy of this future on each runtime. Do this for server listeners
    /// using SO_REUSEADDR.
    #[track_caller]
    pub fn spawn_on_each<F>(&self, future: impl Fn() -> F) -> Vec<JoinHandle<F::Output>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.workers
            .iter()
            .map(|worker| worker.spawn_local(future()))
            .collect()
    }
}

#[track_caller]
fn balance_workers(workers: &[LevelWorkerHandle]) -> &LevelWorkerHandle {
    let mut rng = rand::rng();
    let a = rng.random_range(0..(workers.len()));
    let b = rng.random_range(0..(workers.len()));
    log::info!(
        "{a}: {}, {b}: {}",
        workers[a].concurrency(),
        workers[b].concurrency()
    );
    if workers[a].concurrency() < workers[b].concurrency() {
        &workers[a]
    } else {
        &workers[b]
    }
}

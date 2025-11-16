use std::{future::Future, sync::OnceLock};

use rand::Rng;
use tokio::task::JoinHandle;

use crate::worker::{LevelWorker, LevelWorkerHandle};

static GLOBAL_RUNTIME: OnceLock<LevelRuntimeHandle> = OnceLock::new();

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

pub fn balance() -> LevelWorkerHandle {
    match GLOBAL_RUNTIME
        .get()
        .map(|runtime| balance_workers(&runtime.workers))
    {
        Some(handle) => handle.clone(),
        None => {
            panic!("spawn_balanced can only be called in processes running a default level worker")
        }
    }
}

pub struct LevelRuntime {
    workers: Vec<LevelWorker>,
}
impl LevelRuntime {
    pub(crate) fn from_workers(workers: Vec<LevelWorker>) -> Self {
        Self { workers }
    }

    pub fn set_default(&self) {
        if GLOBAL_RUNTIME.set(self.handle()).is_err() {
            panic!("must only set one default level runtime per process")
        }
    }

    pub fn handle(&self) -> LevelRuntimeHandle {
        LevelRuntimeHandle {
            workers: self.workers.iter().map(LevelWorker::handle).collect(),
        }
    }

    pub fn run(self) {
        let handles: Vec<_> = self
            .workers
            .into_iter()
            .map(|worker| {
                std::thread::Builder::new()
                    .spawn(move || {
                        worker.run();
                    })
                    .expect("must be able to spawn level worker")
            })
            .collect();
        for handle in handles {
            let _ = handle.join();
        }
    }
}

#[derive(Clone)]
pub struct LevelRuntimeHandle {
    workers: Vec<LevelWorkerHandle>,
}

impl LevelRuntimeHandle {
    #[track_caller]
    pub fn spawn_balanced<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        balance_workers(&self.workers).spawn_local(future)
    }

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

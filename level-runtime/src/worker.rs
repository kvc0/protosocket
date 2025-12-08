use std::{
    cell::RefCell,
    future::Future,
    sync::{atomic::AtomicUsize, Arc},
};

use tokio::task::JoinHandle;

use crate::concurrency_tracker::ConcurrencyTracker;

thread_local! {
    static LOCAL_RUNTIME: RefCell<Option<LevelWorkerHandle>> = const { RefCell::new(None) }
}

/// Spawn this future on this local thread runtime.
///
/// You should generally replace your `tokio::spawn` with this. By using
/// `spawn_local` instead of `tokio::spawn`, you give the load heuristic
/// more information.
///
/// If your work truly does not have a thread affinity consideration, use
/// `spawn_balanced` instead.
#[track_caller]
pub fn spawn_local<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match LOCAL_RUNTIME.try_with(|context| {
        context
            .borrow()
            .as_ref()
            .map(|worker| worker.spawn_local(future))
    }) {
        Ok(Some(handle)) => handle,
        Ok(None) => panic!("spawn_local can only be called on threads running a level worker"),
        Err(_access_error) => panic!("spawn_local called on a destroyed thread context"),
    }
}

/// A thread-local runtime wrapper
pub struct LevelWorker {
    runtime: tokio::runtime::Runtime,
    concurrency: Arc<AtomicUsize>,
}
impl LevelWorker {
    pub(crate) fn from_runtime(runtime: tokio::runtime::Runtime) -> Self {
        Self {
            runtime,
            concurrency: Default::default(),
        }
    }

    pub(crate) fn run(&self, termination: impl Future<Output = ()>) {
        LOCAL_RUNTIME.with(|none| {
            assert!(none.borrow().is_none(), "you can't run twice!");
            none.replace(Some(self.handle()));
        });
        assert_eq!(
            LOCAL_RUNTIME.try_with(|a| { a.borrow().is_some() }),
            Ok(true)
        );
        self.runtime.block_on(termination);
        log::debug!("runtime ended");
    }

    /// A spawn handle for the local runtime
    pub fn handle(&self) -> LevelWorkerHandle {
        LevelWorkerHandle {
            handle: self.runtime.handle().clone(),
            concurrency: self.concurrency.clone(),
        }
    }
}

/// A spawn handle for the local runtime
#[derive(Clone)]
pub struct LevelWorkerHandle {
    handle: tokio::runtime::Handle,
    concurrency: Arc<AtomicUsize>,
}

impl LevelWorkerHandle {
    /// Spawn the future on this thread's local runtime
    #[track_caller]
    pub fn spawn_local<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle
            .spawn(ConcurrencyTracker::wrap(self.concurrency.clone(), future))
    }

    /// How many tasks are currently spawned on this level worker?
    pub fn concurrency(&self) -> usize {
        self.concurrency.load(std::sync::atomic::Ordering::Relaxed)
    }
}

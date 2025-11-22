use std::{future::Future, marker::PhantomData};

/// A strategy for spawning futures.
///
/// Notably, Send is not required at the Spawn level. If you have
/// a non-Send service, you can implement your own Spawn.
pub trait Spawn<F>: 'static
where
    F: Future + 'static,
{
    /// Spawn a connection driver task and a connection server task.
    fn spawn(&mut self, future: F);
}

/// When everything in your `SocketService` is `Send`, you can use a TokioSpawn.
#[derive(Debug)]
pub struct TokioSpawn<F> {
    _phantom: PhantomData<F>,
}

impl<F> Default for TokioSpawn<F> {
    fn default() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}
impl<F> Spawn<F> for TokioSpawn<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn spawn(&mut self, driver: F) {
        tokio::spawn(driver);
    }
}

/// When everything in your `SocketService` is `Send`, and you want a connection to be thread-pinned, you can use a LevelSpawnConnection.
#[derive(Debug)]
pub struct LevelSpawn<F> {
    _phantom: PhantomData<F>,
}
impl<F> Default for LevelSpawn<F> {
    fn default() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}
impl<F> Spawn<F> for LevelSpawn<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn spawn(&mut self, future: F) {
        level_runtime::spawn_local(future);
    }
}

#![deny(missing_docs)]
//! A per-thread runtime wrapper for Tokio
//!
//! A level runtime is a multi-thread Tokio configuration,
//! but each thread is its own local runtime. It's mostly
//! for servers.
//!
//! Use `spawn_balanced()` to spawn a task on any thread.
//! Load balancing the tasks across the runtimes is done via
//! best-of-two random choices.
//!
//! Use `spawn_local()` to spawn a task on _this_ thread, with
//! the concurrency tracking intact. Avoid `tokio::spawn`, as
//! that interferes with the load leveling heuristic.
//!
//! Use `spawn_on_each()` to spawn multiple copies of a task,
//! one per worker thread.

mod builder;
mod concurrency_tracker;
mod runtime;
mod worker;

pub use builder::Builder;
pub use runtime::{LevelRuntime, LevelRuntimeHandle};
pub use worker::{LevelWorker, LevelWorkerHandle};

pub use runtime::spawn_balanced;
pub use runtime::spawn_on_each;
pub use worker::spawn_local;

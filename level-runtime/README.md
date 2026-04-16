# level-runtime

A per-thread Tokio runtime wrapper for servers.

Instead of one multi-thread runtime sharing work across threads, `level-runtime` gives each
OS thread its own single-threaded Tokio runtime. Tasks stay on the thread they land on.
`spawn_balanced()` distributes new tasks across threads using a best-of-two random choice,
picking the thread with fewer active tasks.

This avoids the cross-thread synchronization cost of Tokio's work-stealing scheduler, and
gives you more parallel lanes for IO than the typical singleton IO driver. This matters
matters when you have many connections or when you have low latency requirements.

## Usage

```rust
use level_runtime::Builder;

let runtime = Builder::default()
    .worker_threads(4)
    .thread_name_prefix("my-server")
    .enable_all()
    .build();

// Register as the global default so the static spawn functions work
runtime.set_default();

// Spawn one server listener per thread (useful with SO_REUSEPORT)
runtime.handle().spawn_on_each(|| async {
    // run_server_listener().await
});

// Block until a shutdown signal
runtime.run_with_termination(async {
    // watch for ctrl+c or something
    std::future::pending().await
});
```

## Spawning

|      Function       | What it does |
|---------------------|--------------|
| `spawn_local(f)`    | Spawn on the **current** thread's runtime |
| `spawn_balanced(f)` | Spawn on the least-loaded thread (best-of-two random pick) |
| `spawn_on_each(f)`  | Spawn one copy of `f` on every thread |

**Use `spawn_local()` instead of `tokio::spawn()`** when running inside a level worker thread.
`tokio::spawn()` bypasses the concurrency counter, which defeats the load-leveling heuristic.

## Thread naming

Thread names are truncated to 12 characters to stay within the 15-character limit that tools
like `htop` show (the suffix `-NN` takes 3 characters).

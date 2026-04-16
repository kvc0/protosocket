# benchmarks

Criterion benchmark suite for the protosocket workspace.

```bash
cargo bench -p benchmarks
```

## Benches

**`channel`** — Compares `spillway` vs `tokio::mpsc` throughput and latency with 16 concurrent
senders. This is the source of the numbers in `spillway`'s README.

**`safety_perf`** — Compares two approaches to building the `IoSlice` array for vectored writes
(`writev`): heap-allocated `Vec<IoSlice>` vs a stack-allocated array filled via `chunks_vectored`.
This informed the implementation in `protosocket`'s connection write path.
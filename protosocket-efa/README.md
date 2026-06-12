# protosocket-efa

A [`protosocket::SocketListener`] implemented over [libfabric], so protosocket
servers can run on RDMA fabrics — most notably **AWS EFA** (Elastic Fabric
Adapter), but also any other libfabric provider.

## What it does

libfabric's reliable endpoints expose *message* (datagram) semantics with **no
native `connect`/`accept`**. This crate builds a connection-oriented, ordered
byte stream on top of reliable-datagram messaging:

- [`Provider`] selects the libfabric provider: `Efa`, `Tcp`, `Udp`, `Verbs`, or
  `Shm`. `Tcp`/`Udp`/`Shm` need no special hardware and are handy for
  development, testing, and CI.
- Connections are bootstrapped over a **TCP side-channel** that exchanges raw
  fabric addresses (the same approach NCCL and MPI use). Once peers know each
  other's address, data flows over the fabric.
- Each connection is surfaced as an [`EfaStream`], which implements
  `tokio::io::AsyncRead` + `AsyncWrite`, so it plugs straight into protosocket's
  `Connection`.

A **driver** owns the fabric and services every connection by reaping one
completion queue. It runs in one of two modes, chosen automatically per
provider:

- providers whose CQ exposes an awaitable fd (`FI_WAIT_FD` — e.g. `tcp`,
  `verbs`) use an **async task** that awaits the fd via tokio's `AsyncFd` — no
  busy-polling;
- providers without one (`efa`, `udp`, `shm`) use a dedicated **busy-poll
  thread** kept off the tokio workers.

## Server

```rust,no_run
use protosocket_efa::{EfaSocketListener, Provider};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let listener = EfaSocketListener::bind(Provider::Efa, "0.0.0.0:9000".parse()?).await?;
// hand `listener` to a protosocket server as its `SocketListener`.
# Ok(()) }
```

## Client

```rust,no_run
use protosocket_efa::{EfaStream, Provider};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let stream = EfaStream::connect(Provider::Efa, "10.0.0.5:9000".parse()?).await?;
// `stream` is AsyncRead + AsyncWrite.
# Ok(()) }
```

## Requirements

- **libfabric** must be installed to build (the `ofi-libfabric-sys` dependency
  links against it). On Debian/Ubuntu: `apt-get install libfabric-dev`; on
  Arch: `pacman -S libfabric`.
- `Provider::Efa` requires AWS EC2 instances with an Elastic Fabric Adapter and
  the EFA kernel driver installed. The other providers run anywhere libfabric
  supports them.

## Testing

Unit tests (provider mapping, bootstrap framing) need no hardware. The
`loopback` integration test exercises the full stack over the libfabric `tcp`
provider on `127.0.0.1`, so it runs on an ordinary machine with libfabric
installed:

```sh
cargo test -p protosocket-efa
```

To validate the real transport, run on two EFA-equipped EC2 instances with
`Provider::Efa`.

## Status / limitations (first cut)

- Data moves through registered bounce buffers (copy-in/out); a zero-copy /
  RDMA-write fast path is future work.
- Messages carry a 1-byte opcode (data vs. FIN). End-of-stream is signalled by
  an explicit `poll_shutdown`; a dropped stream retires without notifying the
  peer (sending to a departed peer is unsafe on some providers).
- One endpoint (QP) per connection; no multi-rail or NUMA pinning yet.

[libfabric]: https://ofiwg.github.io/libfabric/
[`protosocket::SocketListener`]: https://docs.rs/protosocket
[`Provider`]: https://docs.rs/protosocket-efa
[`EfaStream`]: https://docs.rs/protosocket-efa
[`EfaSocketListener`]: https://docs.rs/protosocket-efa

//! End-to-end loopback tests over host-independent libfabric providers.
//!
//! These exercise the whole stack — bootstrap handshake, endpoint setup, the
//! driver, and the `AsyncRead`/`AsyncWrite` byte path — without EFA hardware,
//! and across both driver backends:
//!
//! - `tcp` has an awaitable CQ fd, so it uses the async (`AsyncFd`) reaper.
//! - `udp`/`shm` have no wait fd, so they use the busy-poll thread reaper — the
//!   same path EFA takes.
//!
//! Run with EFA hardware and `Provider::Efa` to validate the real transport.

use std::future::poll_fn;
use std::task::Poll;
use std::time::Duration;

use protosocket::{SocketListener, SocketResult};
use protosocket_efa::{EfaSocketListener, EfaStream, Provider};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn accept_one(listener: &mut EfaSocketListener) -> EfaStream {
    poll_fn(|cx| match listener.poll_accept(cx) {
        Poll::Ready(SocketResult::Stream(stream)) => Poll::Ready(stream),
        Poll::Ready(SocketResult::Disconnect) => panic!("listener disconnected"),
        Poll::Pending => Poll::Pending,
    })
    .await
}

/// Bind a listener, connect a client, and round-trip a small message both ways.
async fn round_trip(provider: Provider) {
    let _ = env_logger::builder().is_test(true).try_init();
    let bind = "127.0.0.1:0".parse().expect("valid socket address");
    let mut listener = EfaSocketListener::bind(provider, bind)
        .await
        .unwrap_or_else(|e| panic!("bind {provider:?} listener: {e}"));
    let address = listener.local_addr().expect("listener has a local address");

    // Server: accept one connection and echo a 5-byte message back.
    let server = tokio::spawn(async move {
        let mut stream = accept_one(&mut listener).await;
        let mut buf = [0u8; 5];
        stream
            .read_exact(&mut buf)
            .await
            .expect("server reads request");
        stream.write_all(&buf).await.expect("server writes echo");
        stream.flush().await.expect("server flushes echo");
        // Keep the listener (and its driver) alive until the client is done.
        tokio::time::sleep(Duration::from_millis(200)).await;
    });

    // Client: connect, send, and read the echo back.
    let mut client = tokio::time::timeout(
        Duration::from_secs(10),
        EfaStream::connect(provider, address),
    )
    .await
    .expect("client connect did not time out")
    .expect("client connects");

    client.write_all(b"hello").await.expect("client writes");
    client.flush().await.expect("client flushes");

    let mut echo = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(10), client.read_exact(&mut echo))
        .await
        .expect("client read did not time out")
        .expect("client reads echo");

    assert_eq!(&echo, b"hello", "echoed bytes must match");
    server.await.expect("server task completes");
}

/// `tcp` → async (`AsyncFd`) reaper path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_tcp_async_path() {
    round_trip(Provider::Tcp).await;
}

/// `shm` → busy-poll thread reaper path (same backend EFA uses).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_shm_busy_poll_path() {
    round_trip(Provider::Shm).await;
}

/// `udp` → busy-poll thread reaper path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_udp_busy_poll_path() {
    round_trip(Provider::Udp).await;
}

//! The completion driver: it owns the [`Fabric`], reaps its completion queue,
//! and carries bytes to and from each connection's [`StreamConn`].
//!
//! # Ownership, not locking
//!
//! A connection is split into two halves that never share mutable state:
//!
//! - [`StreamConn`] is owned **by value** by an [`EfaStream`](crate::EfaStream)
//!   and is therefore exclusively `&mut` from the single task that drives the
//!   protosocket `Connection`. It holds one end of two [`spillway`] channels.
//! - The reaper owns the matching [`DriverConn`] (endpoint, buffers, the other
//!   channel ends) and is the only thing that touches libfabric.
//!
//! Bytes flow over the channels: outbound (`StreamConn → reaper`) and inbound
//! (`reaper → StreamConn`). Because each half is single-owner, neither side
//! needs a lock. The only cross-thread signals are an [`AtomicWaker`] for
//! write-backpressure readiness and a set-once error latch.
//!
//! # Two reaper backends
//!
//! Completions have to be reaped by reading the CQ; the difference is what wakes
//! the reaper to do so:
//!
//! - [`run_async`]: when the provider's CQ exposes an `FI_WAIT_FD` fd (tcp,
//!   verbs), the reaper is a tokio task that awaits the fd via [`AsyncFd`] —
//!   no busy-poll, fully on the async stack.
//! - [`run_thread`]: when there is no awaitable fd (EFA, udp, shm), the reaper
//!   is a dedicated thread that polls the CQ, parking briefly when idle. RDMA
//!   completion reaping is a busy-poll, and it belongs off the tokio workers.
//!
//! Either way the reaper is woken promptly when a stream queues work or drops.

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::task::AtomicWaker;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;

use super::error::EfaError;
use super::fabric::{Endpoint, Fabric, MemoryRegion, FI_ADDR_UNSPEC};
use super::ffi::fi;

/// Size of each registered send/recv buffer, and the largest chunk a single
/// `poll_write` will accept. Larger writes are split across messages.
const BUFFER_SIZE: usize = 64 * 1024;
/// Number of buffers in each per-connection pool.
const SEND_POOL: usize = 16;
const RECV_POOL: usize = 16;
/// Outbound channel capacity (chunks) — the write-side backpressure bound.
const OUTBOUND_CAPACITY: u64 = 64;
/// Idle park interval for the busy-poll (thread) reaper.
const IDLE_PARK: Duration = Duration::from_micros(100);
/// Safety-net wake interval for the fd-driven (async) reaper, so manual-progress
/// providers still advance connection management even if the fd stays quiet.
const CM_FALLBACK: Duration = Duration::from_millis(1);

/// op_context kind discriminants.
const KIND_SEND: u8 = 0;
const KIND_RECV: u8 = 1;

/// Every fabric message begins with a 1-byte opcode. This keeps the FIN a
/// 1-byte message rather than a zero-length one — some providers (notably shm)
/// fault on zero-length sends, and EFA's support for them is not guaranteed.
const OP_DATA: u8 = 0;
const OP_FIN: u8 = 1;
/// Payload bytes carried per message (buffer minus the 1-byte opcode).
const MAX_PAYLOAD: usize = BUFFER_SIZE - 1;

/// A unit of outbound work from the stream to the driver.
enum Out {
    /// Stream bytes to send.
    Data(Vec<u8>),
    /// The local side finished writing; send a zero-length FIN to the peer.
    Fin,
}

/// A `std::task::Context` with a no-op waker, for non-blocking channel polls on
/// the driver thread (which is woken by `unpark`, not channel wakers).
fn noop_context() -> Context<'static> {
    Context::from_waker(std::task::Waker::noop())
}

// ---------------------------------------------------------------------------
// Stream-owned half
// ---------------------------------------------------------------------------

/// The stream-facing half of a connection, owned by value (and thus `&mut`) by
/// an [`EfaStream`](crate::EfaStream). No `Arc`, no lock.
pub(crate) struct StreamConn {
    /// Stream → driver. Bytes to send, plus a FIN marker.
    outbound: spillway::Sender<Out>,
    /// Driver → stream. Received byte chunks; closes (yields `None`) on EOF.
    inbound: spillway::Receiver<Vec<u8>>,
    /// A partially-consumed inbound chunk: `(bytes, offset_already_read)`.
    partial: Option<(Vec<u8>, usize)>,
    /// Set-once fatal error, shared with the driver.
    error: Arc<OnceLock<String>>,
    /// Woken by the driver when outbound capacity frees up.
    write_waker: Arc<AtomicWaker>,
    /// Keeps the driver thread alive and lets us unpark it.
    driver: Arc<Driver>,
    /// Whether a FIN has already been queued (shutdown is idempotent).
    fin_queued: bool,
}

impl StreamConn {
    /// Drain up to `dst.len()` received bytes into `dst`.
    ///
    /// `Ready(0)` is clean EOF; `Pending` means no bytes are buffered yet.
    pub(crate) fn poll_read(
        &mut self,
        cx: &mut Context<'_>,
        dst: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(error) = self.error.get() {
            return Poll::Ready(Err(std::io::Error::other(error.clone())));
        }
        loop {
            if let Some((chunk, offset)) = &mut self.partial {
                let n = (chunk.len() - *offset).min(dst.len());
                dst[..n].copy_from_slice(&chunk[*offset..*offset + n]);
                *offset += n;
                if *offset >= chunk.len() {
                    self.partial = None;
                }
                return Poll::Ready(Ok(n));
            }
            match self.inbound.poll_next(cx) {
                Poll::Ready(Some(chunk)) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    self.partial = Some((chunk, 0));
                }
                Poll::Ready(None) => {
                    // EOF. Re-check the error latch in case both fired.
                    if let Some(error) = self.error.get() {
                        return Poll::Ready(Err(std::io::Error::other(error.clone())));
                    }
                    return Poll::Ready(Ok(0));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    /// Queue `src` for sending, returning how many bytes were accepted.
    pub(crate) fn poll_write(
        &mut self,
        cx: &mut Context<'_>,
        src: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(error) = self.error.get() {
            return Poll::Ready(Err(std::io::Error::other(error.clone())));
        }
        if src.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let n = src.len().min(BUFFER_SIZE);
        self.offer(cx, Out::Data(src[..n].to_vec()), n)
    }

    /// Vectored variant of [`StreamConn::poll_write`], coalescing into one chunk.
    pub(crate) fn poll_write_vectored(
        &mut self,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        if let Some(error) = self.error.get() {
            return Poll::Ready(Err(std::io::Error::other(error.clone())));
        }
        let mut chunk = Vec::new();
        for buf in bufs {
            if chunk.len() >= BUFFER_SIZE {
                break;
            }
            let take = (BUFFER_SIZE - chunk.len()).min(buf.len());
            chunk.extend_from_slice(&buf[..take]);
        }
        let n = chunk.len();
        if n == 0 {
            return Poll::Ready(Ok(0));
        }
        self.offer(cx, Out::Data(chunk), n)
    }

    /// Bytes are queued the instant `poll_write` accepts them, so flush is a
    /// no-op beyond surfacing a pending error.
    pub(crate) fn poll_flush(&mut self, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.error.get() {
            Some(error) => Poll::Ready(Err(std::io::Error::other(error.clone()))),
            None => Poll::Ready(Ok(())),
        }
    }

    /// Queue a FIN for the peer. Idempotent.
    pub(crate) fn poll_shutdown(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if let Some(error) = self.error.get() {
            return Poll::Ready(Err(std::io::Error::other(error.clone())));
        }
        if self.fin_queued {
            return Poll::Ready(Ok(()));
        }
        match self.offer(cx, Out::Fin, 0) {
            Poll::Ready(Ok(_)) => {
                self.fin_queued = true;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }

    /// Offer one item to the outbound channel, handling backpressure by
    /// registering `write_waker` and retrying once (to close the register/free
    /// race). Returns `Ready(Ok(accepted))` on success.
    fn offer(
        &mut self,
        cx: &mut Context<'_>,
        item: Out,
        accepted: usize,
    ) -> Poll<std::io::Result<usize>> {
        match self.outbound.send(item) {
            Ok(()) => {
                self.driver.wake();
                Poll::Ready(Ok(accepted))
            }
            Err(spillway::Error::Closed(_)) => Poll::Ready(Err(self.closed_error())),
            Err(spillway::Error::Full(item)) => {
                self.write_waker.register(cx.waker());
                match self.outbound.send(item) {
                    Ok(()) => {
                        self.driver.wake();
                        Poll::Ready(Ok(accepted))
                    }
                    Err(spillway::Error::Closed(_)) => Poll::Ready(Err(self.closed_error())),
                    Err(spillway::Error::Full(_)) => Poll::Pending,
                }
            }
        }
    }

    fn closed_error(&self) -> std::io::Error {
        match self.error.get() {
            Some(error) => std::io::Error::other(error.clone()),
            None => std::io::Error::from(std::io::ErrorKind::BrokenPipe),
        }
    }
}

impl Drop for StreamConn {
    fn drop(&mut self) {
        // Dropping `outbound` closes the channel; wake the driver so it notices
        // and retires the connection.
        self.driver.wake();
    }
}

// ---------------------------------------------------------------------------
// Driver-owned half
// ---------------------------------------------------------------------------

/// Per-operation identity passed to libfabric as `op_context` and read back on
/// completion. Boxed and owned by the [`DriverConn`]; the same box is reused on
/// every repost, so its pointer is valid for the life of the connection.
struct OpId {
    conn_id: u64,
    kind: u8,
    buf: u32,
}

/// A registered I/O buffer plus its reused op-context box.
struct IoBuffer {
    // `mr` must drop before `mem` (it references the memory while registered).
    mr: MemoryRegion,
    mem: Box<[u8]>,
    op: Box<OpId>,
    /// For send buffers: whether currently in flight.
    busy: bool,
}

/// The driver-side ends of a connection, handed to the driver thread by
/// [`Driver::activate`].
struct DriverHalf {
    inbound: spillway::Sender<Vec<u8>>,
    outbound: spillway::Receiver<Out>,
    error: Arc<OnceLock<String>>,
    write_waker: Arc<AtomicWaker>,
}

/// Everything the driver needs to service one active connection.
struct DriverConn {
    // `endpoint` drops before the buffers, flushing/cancelling outstanding ops.
    endpoint: Endpoint,
    peer: fi::fi_addr_t,
    /// Driver → stream. `None` once we've signalled EOF (dropped the sender).
    inbound: Option<spillway::Sender<Vec<u8>>>,
    /// Stream → driver.
    outbound: spillway::Receiver<Out>,
    error: Arc<OnceLock<String>>,
    write_waker: Arc<AtomicWaker>,
    send: Vec<IoBuffer>,
    recv: Vec<IoBuffer>,
    /// The outbound chunk currently being sent: `(bytes, offset_already_sent)`.
    leftover: Option<(Vec<u8>, usize)>,
    /// Sends handed to the fabric but not yet completed.
    inflight: usize,
    /// A FIN is queued / has been sent.
    fin_pending: bool,
    fin_sent: bool,
    /// The stream dropped (outbound channel closed); drain then retire.
    closed: bool,
}

impl DriverConn {
    /// Record a fatal error and wake both sides.
    fn fail(&mut self, error: &EfaError) {
        let message = error.to_string();
        log::error!("efa connection failed: {message}");
        let _ = self.error.set(message);
        // Dropping the inbound sender unblocks a parked reader (which then sees
        // the error latch); wake the writer too.
        self.inbound = None;
        self.write_waker.wake();
    }
}

// ---------------------------------------------------------------------------
// Control plane
// ---------------------------------------------------------------------------

/// A control message from an async caller to the driver thread.
enum Control {
    /// Create an endpoint and report its id + raw local address.
    CreateEndpoint {
        reply: tokio::sync::oneshot::Sender<Result<PendingConn, EfaError>>,
    },
    /// Bind a previously-created endpoint to a peer and start data flow.
    Activate {
        conn_id: u64,
        peer_address: Vec<u8>,
        half: DriverHalf,
        reply: tokio::sync::oneshot::Sender<Result<(), EfaError>>,
    },
}

/// A created-but-not-yet-active endpoint, plus its raw local fabric address (to
/// send to the peer over the bootstrap channel).
pub struct PendingConn {
    /// The driver-assigned connection id.
    pub conn_id: u64,
    /// This endpoint's raw local fabric address.
    pub local_address: Vec<u8>,
}

/// How to wake the reaper from an async caller.
enum WakeHandle {
    /// Unpark the busy-poll thread.
    Thread(std::thread::Thread),
    /// Notify the fd-driven async task.
    Task(Arc<Notify>),
}

impl WakeHandle {
    fn wake(&self) {
        match self {
            WakeHandle::Thread(thread) => thread.unpark(),
            WakeHandle::Task(notify) => notify.notify_one(),
        }
    }
}

/// How to stop and reclaim the reaper when the `Driver` drops.
enum Shutdown {
    Thread(Mutex<Option<std::thread::JoinHandle<()>>>),
    Task(Mutex<Option<tokio::task::JoinHandle<()>>>),
}

/// Shared handle to the reaper. Held by the listener and by every [`StreamConn`]
/// (but **not** by the reaper itself, so this can reach refcount zero and stop
/// the reaper). The reaper is one of two backends:
///
/// - a busy-poll **thread** for providers whose CQ has no awaitable fd (EFA,
///   udp, shm), or
/// - an fd-driven **async task** for providers that support `FI_WAIT_FD` (tcp,
///   verbs), which integrates with tokio and never busy-spins.
pub struct Driver {
    control: UnboundedSender<Control>,
    wake_handle: WakeHandle,
    shutdown: Shutdown,
    /// Signals the reaper to stop. Shared with the reaper (not via `Driver`, to
    /// avoid a reference cycle).
    stop: Arc<AtomicBool>,
}

impl Driver {
    /// Start a reaper owning `fabric`, choosing the async (fd) path when the
    /// provider supports it and falling back to a busy-poll thread otherwise.
    pub fn start(fabric: Fabric) -> Arc<Driver> {
        let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));

        // Async path: only when the CQ exposes an fd and we can register it.
        if let Some(fd) = fabric.wait_fd() {
            match AsyncFd::new(fd) {
                Ok(async_fd) => {
                    let notify = Arc::new(Notify::new());
                    let task = tokio::spawn(run_async(
                        fabric,
                        control_rx,
                        Arc::clone(&stop),
                        Arc::clone(&notify),
                        async_fd,
                    ));
                    return Arc::new(Driver {
                        control: control_tx,
                        wake_handle: WakeHandle::Task(notify),
                        shutdown: Shutdown::Task(Mutex::new(Some(task))),
                        stop,
                    });
                }
                Err(e) => {
                    log::warn!("AsyncFd registration failed ({e}); using busy-poll thread");
                    // `fabric` was not moved (AsyncFd::new took the fd, a copy);
                    // fall through to the thread path.
                }
            }
        }

        // Busy-poll thread path.
        let stop_for_thread = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("protosocket-efa-driver".to_string())
            .spawn(move || run_thread(fabric, control_rx, stop_for_thread))
            .expect("failed to spawn efa driver thread");
        let wake_handle = WakeHandle::Thread(join.thread().clone());
        Arc::new(Driver {
            control: control_tx,
            wake_handle,
            shutdown: Shutdown::Thread(Mutex::new(Some(join))),
            stop,
        })
    }

    /// Wake the reaper to do work.
    fn wake(&self) {
        self.wake_handle.wake();
    }

    fn send_control(&self, control: Control) -> Result<(), EfaError> {
        self.control
            .send(control)
            .map_err(|_| EfaError::Bootstrap("efa driver is gone".to_string()))?;
        self.wake();
        Ok(())
    }

    /// Create an endpoint and learn its local address (driver-thread work).
    pub async fn create_endpoint(&self) -> Result<PendingConn, EfaError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.send_control(Control::CreateEndpoint { reply })?;
        rx.await
            .map_err(|_| EfaError::Bootstrap("efa driver dropped reply".to_string()))?
    }

    /// Bind `pending` to `peer_address`, begin data flow, and return the
    /// stream-side handle.
    pub async fn activate(
        self: &Arc<Self>,
        pending: PendingConn,
        peer_address: Vec<u8>,
    ) -> Result<StreamConn, EfaError> {
        let (out_tx, out_rx) =
            spillway::channel_with_capacity_and_concurrency(OUTBOUND_CAPACITY, 1);
        let (in_tx, in_rx) = spillway::channel_with_concurrency(1);
        let error = Arc::new(OnceLock::new());
        let write_waker = Arc::new(AtomicWaker::new());

        let half = DriverHalf {
            inbound: in_tx,
            outbound: out_rx,
            error: Arc::clone(&error),
            write_waker: Arc::clone(&write_waker),
        };
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.send_control(Control::Activate {
            conn_id: pending.conn_id,
            peer_address,
            half,
            reply,
        })?;
        rx.await
            .map_err(|_| EfaError::Bootstrap("efa driver dropped reply".to_string()))??;

        Ok(StreamConn {
            outbound: out_tx,
            inbound: in_rx,
            partial: None,
            error,
            write_waker,
            driver: Arc::clone(self),
            fin_queued: false,
        })
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake();
        match &self.shutdown {
            Shutdown::Thread(join) => {
                if let Some(handle) = join.lock().expect("join mutex").take() {
                    let _ = handle.join();
                }
            }
            Shutdown::Task(task) => {
                // Can't join an async task from a sync drop; the stop flag + wake
                // let it exit on its own, and abort backstops a stuck await.
                if let Some(handle) = task.lock().expect("join mutex").take() {
                    handle.abort();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Driver thread
// ---------------------------------------------------------------------------

/// Build a per-connection buffer pool, registering each buffer.
fn build_buffers(
    fabric: &Fabric,
    conn_id: u64,
    kind: u8,
    count: usize,
) -> Result<Vec<IoBuffer>, EfaError> {
    let mut buffers = Vec::with_capacity(count);
    for index in 0..count {
        let mem = vec![0u8; BUFFER_SIZE].into_boxed_slice();
        let mr = fabric.register(&mem)?;
        buffers.push(IoBuffer {
            mr,
            mem,
            op: Box::new(OpId {
                conn_id,
                kind,
                buf: index as u32,
            }),
            busy: false,
        });
    }
    Ok(buffers)
}

/// The reaper's bookkeeping, shared by both backends.
struct Reaper {
    pending: HashMap<u64, Endpoint>,
    conns: HashMap<u64, DriverConn>,
    next_id: u64,
}

impl Reaper {
    fn new() -> Self {
        Reaper {
            pending: HashMap::new(),
            conns: HashMap::new(),
            next_id: 1,
        }
    }
}

/// One unit of reaper work: drain control, service outbound, reap completions,
/// retire finished connections. Returns whether anything happened.
fn pump(fabric: &Fabric, control: &mut UnboundedReceiver<Control>, reaper: &mut Reaper) -> bool {
    let mut did_work = false;

    while let Ok(message) = control.try_recv() {
        did_work = true;
        handle_control(fabric, reaper, message);
    }

    let mut to_retire = Vec::new();
    for (&conn_id, conn) in reaper.conns.iter_mut() {
        match service_outbound(conn) {
            ServiceOutcome::DidWork => did_work = true,
            ServiceOutcome::Idle => {}
            ServiceOutcome::Retire => to_retire.push(conn_id),
        }
    }

    if reap_completions(fabric, &mut reaper.conns) {
        did_work = true;
    }

    for conn_id in to_retire {
        reaper.conns.remove(&conn_id);
    }

    did_work
}

/// Busy-poll reaper backend, for providers with no awaitable CQ fd.
fn run_thread(fabric: Fabric, mut control: UnboundedReceiver<Control>, stop: Arc<AtomicBool>) {
    let mut reaper = Reaper::new();
    while !stop.load(Ordering::SeqCst) {
        if !pump(&fabric, &mut control, &mut reaper) {
            std::thread::park_timeout(IDLE_PARK);
        }
    }
}

/// fd-driven reaper backend: awaits CQ readiness via `AsyncFd` instead of
/// busy-polling, so it lives on the tokio runtime.
async fn run_async(
    fabric: Fabric,
    mut control: UnboundedReceiver<Control>,
    stop: Arc<AtomicBool>,
    notify: Arc<Notify>,
    async_fd: AsyncFd<RawFd>,
) {
    let mut reaper = Reaper::new();
    while !stop.load(Ordering::SeqCst) {
        pump(&fabric, &mut control, &mut reaper);

        // libfabric's fd-wait protocol: only block on the fd if `fi_trywait`
        // says the CQ is quiescent; otherwise loop to drain pending completions.
        match fabric.try_wait() {
            Ok(true) => {
                tokio::select! {
                    biased;
                    // Outbound work / control / shutdown.
                    _ = notify.notified() => {}
                    // CQ became readable.
                    readable = async_fd.readable() => {
                        if let Ok(mut guard) = readable {
                            guard.clear_ready();
                        }
                    }
                    // Safety net so manual-progress CM still advances.
                    _ = tokio::time::sleep(CM_FALLBACK) => {}
                }
            }
            Ok(false) => {
                // Completions are pending; yield and loop to drain them.
                tokio::task::yield_now().await;
            }
            Err(e) => {
                log::error!("fi_trywait failed: {e}");
                tokio::time::sleep(CM_FALLBACK).await;
            }
        }
    }
    // Deregister the fd from the reactor before `fabric` closes it.
    drop(async_fd);
}

fn handle_control(fabric: &Fabric, reaper: &mut Reaper, message: Control) {
    match message {
        Control::CreateEndpoint { reply } => {
            let result = fabric.create_endpoint().map(|(endpoint, local_address)| {
                let conn_id = reaper.next_id;
                reaper.next_id += 1;
                reaper.pending.insert(conn_id, endpoint);
                PendingConn {
                    conn_id,
                    local_address,
                }
            });
            let _ = reply.send(result);
        }
        Control::Activate {
            conn_id,
            peer_address,
            half,
            reply,
        } => {
            let result = activate_connection(
                fabric,
                &mut reaper.pending,
                &mut reaper.conns,
                conn_id,
                &peer_address,
                half,
            );
            let _ = reply.send(result);
        }
    }
}

fn activate_connection(
    fabric: &Fabric,
    pending: &mut HashMap<u64, Endpoint>,
    conns: &mut HashMap<u64, DriverConn>,
    conn_id: u64,
    peer_address: &[u8],
    half: DriverHalf,
) -> Result<(), EfaError> {
    let endpoint = pending
        .remove(&conn_id)
        .ok_or_else(|| EfaError::Bootstrap("activate for unknown connection".to_string()))?;
    let peer = fabric.insert_address(peer_address)?;
    let send = build_buffers(fabric, conn_id, KIND_SEND, SEND_POOL)?;
    let recv = build_buffers(fabric, conn_id, KIND_RECV, RECV_POOL)?;

    let mut conn = DriverConn {
        endpoint,
        peer,
        inbound: Some(half.inbound),
        outbound: half.outbound,
        error: half.error,
        write_waker: half.write_waker,
        send,
        recv,
        leftover: None,
        inflight: 0,
        fin_pending: false,
        fin_sent: false,
        closed: false,
    };
    for index in 0..conn.recv.len() {
        if let Err(e) = post_recv(&mut conn, index) {
            conn.fail(&e);
            return Err(e);
        }
    }
    log::debug!(
        "activated efa connection {conn_id}: posted {} recv buffers",
        conn.recv.len()
    );
    conns.insert(conn_id, conn);
    Ok(())
}

enum ServiceOutcome {
    DidWork,
    Idle,
    Retire,
}

/// Pull outbound work and post sends; report whether the connection can retire.
fn service_outbound(conn: &mut DriverConn) -> ServiceOutcome {
    let mut did_work = false;

    loop {
        // 1. If we have no chunk in flight, pull the next outbound item.
        if conn.leftover.is_none() && !conn.fin_pending {
            match conn.outbound.poll_next(&mut noop_context()) {
                Poll::Ready(Some(Out::Data(bytes))) => {
                    conn.write_waker.wake(); // freed a channel slot
                    if !bytes.is_empty() {
                        conn.leftover = Some((bytes, 0));
                    }
                    did_work = true;
                }
                Poll::Ready(Some(Out::Fin)) => {
                    conn.write_waker.wake();
                    conn.fin_pending = true;
                    did_work = true;
                }
                Poll::Ready(None) => {
                    // All senders dropped: the stream is gone. Flush whatever is
                    // queued, then retire. We deliberately do NOT synthesize a
                    // FIN here: by the time a stream is dropped the peer may have
                    // departed, and a fabric send to a gone peer is unsafe on
                    // some providers (shm faults). A FIN is only sent on an
                    // explicit `poll_shutdown`, when the peer is expected alive.
                    conn.closed = true;
                }
                Poll::Pending => {}
            }
        }

        // 2. Decide what to post and copy it into a free send buffer.
        let plan = match plan_send(conn) {
            Some(plan) => plan,
            None => break,
        };

        // 3. Post it.
        match post_send(conn, plan.index, plan.wire_len) {
            Ok(()) => {
                did_work = true;
                conn.inflight += 1;
                if plan.is_fin {
                    conn.fin_sent = true;
                } else {
                    advance_leftover(conn, plan.payload_len);
                }
            }
            Err(EfaError::Libfabric { code, .. }) if code == fi::FI_EAGAIN as i32 => {
                // Fabric momentarily full / still connecting: free the buffer and
                // retry next tick. The chunk stays in `leftover`.
                conn.send[plan.index].busy = false;
                break;
            }
            Err(e) => {
                conn.fail(&e);
                break;
            }
        }
    }

    if conn.closed
        && conn.leftover.is_none()
        && conn.inflight == 0
        && (!conn.fin_pending || conn.fin_sent)
    {
        ServiceOutcome::Retire
    } else if did_work {
        ServiceOutcome::DidWork
    } else {
        ServiceOutcome::Idle
    }
}

/// A planned send. `wire_len` is what goes on the fabric (opcode + payload);
/// `payload_len` is how far to advance the outbound stream on success.
struct SendPlan {
    index: usize,
    wire_len: usize,
    payload_len: usize,
    is_fin: bool,
}

/// Choose the next send and frame it (opcode + payload) into a free send buffer,
/// leaving the outbound state otherwise untouched. `None` if nothing to send /
/// no buffer.
fn plan_send(conn: &mut DriverConn) -> Option<SendPlan> {
    if let Some((bytes, offset)) = &conn.leftover {
        let index = conn.send.iter().position(|b| !b.busy)?;
        let payload_len = (bytes.len() - offset).min(MAX_PAYLOAD);
        let (start, end) = (*offset, *offset + payload_len);
        // Disjoint field borrows: read `leftover`, write `send`.
        let buf = &mut conn.send[index].mem;
        buf[0] = OP_DATA;
        buf[1..1 + payload_len].copy_from_slice(&bytes[start..end]);
        Some(SendPlan {
            index,
            wire_len: payload_len + 1,
            payload_len,
            is_fin: false,
        })
    } else if conn.fin_pending && !conn.fin_sent {
        let index = conn.send.iter().position(|b| !b.busy)?;
        conn.send[index].mem[0] = OP_FIN;
        Some(SendPlan {
            index,
            wire_len: 1,
            payload_len: 0,
            is_fin: true,
        })
    } else {
        None
    }
}

/// Advance the leftover chunk by `len` bytes sent, clearing it when drained.
fn advance_leftover(conn: &mut DriverConn, len: usize) {
    if let Some((bytes, offset)) = &mut conn.leftover {
        *offset += len;
        if *offset >= bytes.len() {
            conn.leftover = None;
        }
    }
}

/// Post a send of `len` bytes from send buffer `index`.
fn post_send(conn: &mut DriverConn, index: usize, len: usize) -> Result<(), EfaError> {
    let ep = conn.endpoint.as_ptr();
    let peer = conn.peer;
    let buf = &mut conn.send[index];
    buf.busy = true;
    let op_ctx = (&*buf.op as *const OpId) as *mut c_void;
    // SAFETY: ep is live; buf.mem/desc are registered and stay valid until the
    // send completes (the buffer is busy until then); op_ctx is a box owned by
    // this DriverConn.
    let ret = unsafe {
        fi::fi_send(
            ep,
            buf.mem.as_ptr() as *const c_void,
            len,
            buf.mr.desc(),
            peer,
            op_ctx,
        )
    };
    if ret < 0 {
        return Err(EfaError::libfabric("fi_send", ret as i32));
    }
    log::trace!("posted fi_send of {len} bytes on conn {}", buf.op.conn_id);
    Ok(())
}

/// Post receive buffer `index`.
fn post_recv(conn: &mut DriverConn, index: usize) -> Result<(), EfaError> {
    let ep = conn.endpoint.as_ptr();
    let buf = &mut conn.recv[index];
    let op_ctx = (&*buf.op as *const OpId) as *mut c_void;
    let len = buf.mem.len();
    let desc = buf.mr.desc();
    let ptr = buf.mem.as_mut_ptr();
    // SAFETY: ep is live; buf.mem is registered and owned by this DriverConn for
    // the duration of the recv; op_ctx is a box owned by this DriverConn.
    let ret = unsafe { fi::fi_recv(ep, ptr as *mut c_void, len, desc, FI_ADDR_UNSPEC, op_ctx) };
    if ret < 0 {
        return Err(EfaError::libfabric("fi_recv", ret as i32));
    }
    Ok(())
}

/// Read the shared CQ and route completions. Returns whether anything happened.
fn reap_completions(fabric: &Fabric, conns: &mut HashMap<u64, DriverConn>) -> bool {
    const BATCH: usize = 32;
    // SAFETY: fi_cq_msg_entry is a POD struct; a zeroed array is a valid init.
    let mut entries: [fi::fi_cq_msg_entry; BATCH] = unsafe { std::mem::zeroed() };
    let cq = fabric.cq();

    // SAFETY: cq is live; entries is a writable array of `BATCH` msg entries.
    let read = unsafe { fi::fi_cq_read(cq, entries.as_mut_ptr() as *mut c_void, BATCH) };

    if read > 0 {
        for entry in entries.iter().take(read as usize) {
            route_completion(conns, entry);
        }
        return true;
    }

    let err = -(read as i32);
    if err == fi::FI_EAGAIN as i32 {
        return false;
    }
    if err == fi::FI_EAVAIL as i32 {
        drain_cq_error(fabric, conns);
        return true;
    }
    if read < 0 {
        log::error!("fi_cq_read failed: {read}");
    }
    false
}

/// Decode a completion's op_context and dispatch it to its connection.
fn route_completion(conns: &mut HashMap<u64, DriverConn>, entry: &fi::fi_cq_msg_entry) {
    if entry.op_context.is_null() {
        return;
    }
    // SAFETY: op_context points to an OpId box owned by a live DriverConn (the
    // endpoint, and thus its ops, is only closed after retirement).
    let op = unsafe { &*(entry.op_context as *const OpId) };
    let Some(conn) = conns.get_mut(&op.conn_id) else {
        return;
    };
    log::trace!(
        "completion on conn {} kind {} buf {} len {}",
        op.conn_id,
        op.kind,
        op.buf,
        entry.len
    );
    match op.kind {
        KIND_SEND => complete_send(conn, op.buf as usize),
        KIND_RECV => complete_recv(conn, op.buf as usize, entry.len),
        _ => {}
    }
}

/// A send finished: free its buffer so it can be reused.
fn complete_send(conn: &mut DriverConn, index: usize) {
    conn.send[index].busy = false;
    conn.inflight = conn.inflight.saturating_sub(1);
}

/// A receive finished: forward the bytes (or signal EOF) and repost the buffer.
///
/// Every message is framed `[opcode, payload...]`, so a well-formed message is
/// at least 1 byte; an `OP_FIN` is exactly 1 byte.
fn complete_recv(conn: &mut DriverConn, index: usize, len: usize) {
    if len == 0 {
        // Unframed/empty completion: shouldn't happen, but repost defensively.
        let _ = post_recv(conn, index);
        return;
    }
    let opcode = conn.recv[index].mem[0];
    if opcode == OP_FIN {
        // The peer's FIN. Drop the inbound sender to give the reader a clean EOF.
        conn.inbound = None;
        return;
    }
    let Some(sender) = conn.inbound.as_ref() else {
        return; // already EOF/closed; ignore late data
    };
    // OP_DATA: the payload follows the opcode byte.
    let chunk = conn.recv[index].mem[1..len].to_vec();
    if sender.send(chunk).is_err() {
        // Reader dropped: nothing more to deliver.
        conn.closed = true;
        return;
    }
    if let Err(e) = post_recv(conn, index) {
        conn.fail(&e);
    }
}

/// Drain a CQ error entry and fail the affected connection.
fn drain_cq_error(fabric: &Fabric, conns: &mut HashMap<u64, DriverConn>) {
    // SAFETY: fi_cq_err_entry is POD; zeroed is a valid init.
    let mut err_entry: fi::fi_cq_err_entry = unsafe { std::mem::zeroed() };
    // SAFETY: cq is live; err_entry is writable.
    let ret = unsafe { fi::fi_cq_readerr(fabric.cq(), &mut err_entry, 0) };
    if ret < 0 {
        log::error!("fi_cq_readerr failed: {ret}");
        return;
    }
    if err_entry.op_context.is_null() {
        return;
    }
    // SAFETY: see route_completion.
    let op = unsafe { &*(err_entry.op_context as *const OpId) };
    if let Some(conn) = conns.get_mut(&op.conn_id) {
        if op.kind == KIND_SEND {
            conn.send[op.buf as usize].busy = false;
            conn.inflight = conn.inflight.saturating_sub(1);
        }
        conn.fail(&EfaError::libfabric("completion", -err_entry.err));
    }
}

use std::{
    future::Future,
    pin::{pin, Pin},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use futures::{task::AtomicWaker, Stream};

/// A pooled rpc completion: unary and streaming rpcs presented as one shape.
///
/// A unary rpc is a stream that yields exactly one terminal `Complete` event. A streaming
/// rpc yields `Item` events until the underlying stream finishes with a terminal `Finished`.
/// Cancellation yields a terminal `Cancelled` from either kind. After a terminal event the
/// stream yields `None`.
#[derive(Debug)]
pub struct RpcStream<F, S> {
    id: u64,
    aborted: Arc<AtomicBool>,
    waker: Arc<AtomicWaker>,
    kind: RpcStreamKind<F, S>,
}

#[derive(Debug)]
enum RpcStreamKind<F, S> {
    Unary(F),
    Streaming(S),
    Done,
}

/// An event from a pooled rpc, tagged with terminal-ness so the caller knows what, if
/// anything, to put on the wire after it.
#[derive(Debug)]
pub enum RpcStreamEvent<T> {
    /// A streaming rpc produced a response item.
    Item(T),
    /// A unary rpc completed with its single response. Terminal; the response is the
    /// completion, no trailing control message belongs on the wire.
    Complete(T),
    /// A streaming rpc's stream finished. Terminal; the peer should be told the rpc ended.
    Finished,
    /// The rpc was cancelled. Terminal.
    Cancelled,
}

impl<F, S> RpcStream<F, S> {
    pub fn new_unary(id: u64, completion: F) -> (Self, RpcAbortHandle) {
        Self::new(id, RpcStreamKind::Unary(completion))
    }

    pub fn new_streaming(id: u64, stream: S) -> (Self, RpcAbortHandle) {
        Self::new(id, RpcStreamKind::Streaming(stream))
    }

    fn new(id: u64, kind: RpcStreamKind<F, S>) -> (Self, RpcAbortHandle) {
        let aborted = Arc::new(AtomicBool::new(false));
        let waker = Arc::new(AtomicWaker::new());
        (
            Self {
                id,
                aborted: aborted.clone(),
                waker: waker.clone(),
                kind,
            },
            RpcAbortHandle { aborted, waker },
        )
    }
}

impl<F, S, T> Stream for RpcStream<F, S>
where
    F: Future<Output = T> + Unpin,
    S: Stream<Item = T> + Unpin,
{
    type Item = (u64, RpcStreamEvent<T>);

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let id = self.id;
        self.waker.register(context.waker());
        if self.aborted.load(Ordering::Relaxed) {
            return if matches!(self.kind, RpcStreamKind::Done) {
                Poll::Ready(None)
            } else {
                self.kind = RpcStreamKind::Done;
                Poll::Ready(Some((id, RpcStreamEvent::Cancelled)))
            };
        }
        match &mut self.kind {
            RpcStreamKind::Unary(completion) => match pin!(completion).poll(context) {
                Poll::Ready(response) => {
                    self.kind = RpcStreamKind::Done;
                    Poll::Ready(Some((id, RpcStreamEvent::Complete(response))))
                }
                Poll::Pending => Poll::Pending,
            },
            RpcStreamKind::Streaming(stream) => match pin!(stream).poll_next(context) {
                Poll::Ready(Some(item)) => Poll::Ready(Some((id, RpcStreamEvent::Item(item)))),
                Poll::Ready(None) => {
                    self.kind = RpcStreamKind::Done;
                    Poll::Ready(Some((id, RpcStreamEvent::Finished)))
                }
                Poll::Pending => Poll::Pending,
            },
            RpcStreamKind::Done => Poll::Ready(None),
        }
    }
}

/// Cancels a pooled rpc. The rpc yields a terminal `Cancelled` event on its next poll.
#[derive(Debug)]
pub struct RpcAbortHandle {
    aborted: Arc<AtomicBool>,
    waker: Arc<AtomicWaker>,
}

impl RpcAbortHandle {
    /// Mark the rpc aborted and wake it so the cancellation is observed.
    ///
    /// This races with normal rpc completion. If an rpc has already completed
    /// by the time it observes cancel, it doesn't also cancel itself.
    pub fn mark_aborted(self) {
        self.aborted.store(true, Ordering::Relaxed);
        self.waker.wake();
    }
}

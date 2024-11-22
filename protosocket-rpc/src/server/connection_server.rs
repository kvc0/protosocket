use std::{
    collections::HashMap,
    future::Future,
    pin::{pin, Pin},
    task::{Context, Poll},
};

use futures::{
    stream::{FuturesUnordered, SelectAll},
    Stream,
};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::{server::RpcKind, Error, Message, ProtosocketControlCode};

use super::{
    abortable::{AbortableState, IdentifiableAbortHandle, IdentifiableAbortable},
    ConnectionService,
};

#[derive(Debug)]
pub struct RpcConnectionServer<TConnectionServer>
where
    TConnectionServer: ConnectionService,
{
    connection_server: TConnectionServer,
    inbound: mpsc::UnboundedReceiver<<TConnectionServer as ConnectionService>::Request>,
    outbound: PollSender<<TConnectionServer as ConnectionService>::Response>,
    next_messages_buffer: Vec<<TConnectionServer as ConnectionService>::Request>,
    outstanding_unary_rpcs:
        FuturesUnordered<IdentifiableAbortable<TConnectionServer::UnaryFutureType>>,
    outstanding_streaming_rpcs: SelectAll<IdentifiableAbortable<TConnectionServer::StreamType>>,
    aborts: HashMap<u64, IdentifiableAbortHandle>,
}

impl<TConnectionServer> Future for RpcConnectionServer<TConnectionServer>
where
    TConnectionServer: ConnectionService,
{
    type Output = Result<(), crate::Error>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // receive new messages
        if let Some(early_out) = self.as_mut().poll_receive_buffer(context) {
            return early_out;
        }
        // either we're pending on inbound or we're awake
        self.as_mut().handle_message_buffer();

        // retire and advance outstanding rpcs
        if let Some(early_out) = self.as_mut().poll_advance_unary_rpcs(context) {
            return early_out;
        }
        if let Some(early_out) = self.poll_advance_streaming_rpcs(context) {
            return early_out;
        }

        Poll::Pending
    }
}

impl<TConnectionServer> RpcConnectionServer<TConnectionServer>
where
    TConnectionServer: ConnectionService,
{
    pub fn new(
        connection_server: TConnectionServer,
        inbound: mpsc::UnboundedReceiver<<TConnectionServer as ConnectionService>::Request>,
        outbound: mpsc::Sender<<TConnectionServer as ConnectionService>::Response>,
    ) -> Self {
        Self {
            connection_server,
            inbound,
            outbound: PollSender::new(outbound),
            next_messages_buffer: Default::default(),
            outstanding_unary_rpcs: Default::default(),
            outstanding_streaming_rpcs: Default::default(),
            aborts: Default::default(),
        }
    }

    fn poll_advance_unary_rpcs(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), Error>>> {
        loop {
            match pin!(&mut self.outbound).poll_reserve(context) {
                Poll::Ready(Ok(())) => {
                    // ready to send
                }
                Poll::Ready(Err(_)) => {
                    log::debug!("outbound connection is closed");
                    return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                }
                Poll::Pending => {
                    log::debug!("no room in outbound connection");
                    break;
                }
            }

            match pin!(&mut self.outstanding_unary_rpcs).poll_next(context) {
                Poll::Ready(unary_done) => {
                    match unary_done {
                        Some((id, AbortableState::Ready(Ok(response)))) => {
                            self.aborts.remove(&id);
                            if let Err(_e) = self.outbound.send_item(response) {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                            }
                        }
                        Some((id, AbortableState::Ready(Err(e)))) => {
                            let abort = self.aborts.remove(&id);
                            match e {
                                Error::IoFailure(error) => {
                                    log::warn!("{id} io failure while servicing rpc: {error:?}");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                Error::CancelledRemotely => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                Error::ConnectionIsClosed => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                Error::Finished => {
                                    log::debug!("{id} unary rpc ended");
                                    if let Some(abort) = abort {
                                        if let Err(_e) = self.outbound.send_item(
                                            <TConnectionServer::Response as Message>::ended(id),
                                        ) {
                                            log::debug!("outbound connection is closed");
                                            return Some(Poll::Ready(Err(
                                                crate::Error::ConnectionIsClosed,
                                            )));
                                        }

                                        abort.mark_aborted();
                                    }
                                }
                            }
                            // cancelled
                        }
                        Some((id, AbortableState::Abort)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} unary rpc abort");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.abort();
                            }
                        }
                        Some((id, AbortableState::Aborted)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} unary rpc done");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.mark_aborted();
                            }
                        }
                        None => {
                            // nothing to wait for
                            break;
                        }
                    }
                }
                Poll::Pending => break,
            }
        }
        None
    }

    // I want to join this with the above function but it is annoying to zip the SelectAll and FuturesUnordered together.
    // This should be possible today with futures::Stream but I need to sit and stare at it for a while to figure out how.
    fn poll_advance_streaming_rpcs(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), Error>>> {
        loop {
            match pin!(&mut self.outbound).poll_reserve(context) {
                Poll::Ready(Ok(())) => {
                    // ready to send
                }
                Poll::Ready(Err(_)) => {
                    log::debug!("outbound connection is closed");
                    return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                }
                Poll::Pending => {
                    log::debug!("no room in outbound connection");
                    break;
                }
            }

            match pin!(&mut self.outstanding_streaming_rpcs).poll_next(context) {
                Poll::Ready(streaming_next) => {
                    match streaming_next {
                        Some((id, AbortableState::Ready(Ok(next)))) => {
                            log::debug!("{id} streaming rpc next {next:?}");
                            if let Err(_e) = self.outbound.send_item(next) {
                                log::debug!("outbound connection is closed");
                                return Some(Poll::Ready(Err(crate::Error::ConnectionIsClosed)));
                            }
                        }
                        Some((id, AbortableState::Ready(Err(e)))) => {
                            let abort = self.aborts.remove(&id);
                            match e {
                                Error::IoFailure(error) => {
                                    log::warn!("{id} io failure while servicing rpc: {error:?}");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                Error::CancelledRemotely => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                Error::ConnectionIsClosed => {
                                    log::debug!("{id} rpc cancelled remotely");
                                    if let Some(abort) = abort {
                                        abort.abort();
                                    }
                                }
                                Error::Finished => {
                                    log::debug!("{id} streaming rpc ended");
                                    if let Some(abort) = abort {
                                        if let Err(_e) = self.outbound.send_item(
                                            <TConnectionServer::Response as Message>::ended(id),
                                        ) {
                                            log::debug!("outbound connection is closed");
                                            return Some(Poll::Ready(Err(
                                                crate::Error::ConnectionIsClosed,
                                            )));
                                        }
                                        abort.mark_aborted();
                                    }
                                }
                            }
                        }
                        Some((id, AbortableState::Abort)) => {
                            // This happens when the upstream stuff is dropped and there are no messages that can be produced. We'll send a cancellation.
                            log::debug!("{id} streaming rpc abort");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.abort();
                            }
                        }
                        Some((id, AbortableState::Aborted)) => {
                            log::debug!("{id} streaming rpc done");
                            if let Some(abort) = self.aborts.remove(&id) {
                                abort.mark_aborted();
                            }
                        }
                        None => {
                            // nothing to wait for
                            break;
                        }
                    }
                }
                Poll::Pending => break,
            }
        }
        None
    }

    fn poll_receive_buffer(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Option<Poll<Result<(), Error>>> {
        const MAXIMUM_MESSAGES_PER_POLL: usize = 128;
        if self.next_messages_buffer.is_empty() {
            let Self {
                inbound,
                next_messages_buffer,
                ..
            } = &mut *self;
            match inbound.poll_recv_many(context, next_messages_buffer, MAXIMUM_MESSAGES_PER_POLL) {
                Poll::Ready(0) => {
                    return Some(Poll::Ready(Ok(())));
                }
                Poll::Ready(_count) => {
                    // ugh, I know. but poll_recv_many is much cheaper than poll_recv,
                    // and poll_recv requires &mut Vec. Otherwise this would be a VecDeque with no reverse.
                    next_messages_buffer.reverse();
                    // possible there is more, but let's just do one batch at a time
                    context.waker().wake_by_ref();
                }
                Poll::Pending => {}
            }
        }
        None
    }

    fn handle_message_buffer(mut self: Pin<&mut Self>) {
        while let Some(next_message) = self.next_messages_buffer.pop() {
            let message_id = next_message.message_id();
            match next_message.control_code() {
                ProtosocketControlCode::Normal => {
                    match self.connection_server.new_rpc(next_message) {
                        RpcKind::Unary(completion) => {
                            let (completion, abort) =
                                IdentifiableAbortable::new(message_id, completion);
                            self.aborts.insert(message_id, abort);
                            self.outstanding_unary_rpcs.push(completion);
                        }
                        RpcKind::Streaming(completion) => {
                            let (completion, abort) =
                                IdentifiableAbortable::new(message_id, completion);
                            self.aborts.insert(message_id, abort);
                            self.outstanding_streaming_rpcs.push(completion);
                        }
                        RpcKind::Unknown => {
                            log::debug!("skipping message {message_id}");
                        }
                    }
                }
                ProtosocketControlCode::Cancel => {
                    if let Some(abort) = self.aborts.remove(&message_id) {
                        log::debug!("cancelling message {message_id}");
                        abort.mark_aborted();
                    } else {
                        log::debug!("received cancellation for untracked message {message_id}");
                    }
                }
                ProtosocketControlCode::End => {
                    log::debug!("received end message {message_id}");
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::{
        future::Future,
        pin::pin,
        ptr,
        task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    };

    use futures::{FutureExt, StreamExt};
    use tokio::sync::mpsc;

    use crate::{
        server::{ConnectionService, RpcKind},
        ProtosocketControlCode,
    };

    use super::RpcConnectionServer;

    #[derive(Clone, PartialEq, Eq, prost::Message, PartialOrd, Ord)]
    pub struct Message {
        #[prost(uint64, tag = "1")]
        pub id: u64,
        #[prost(uint32, tag = "2")]
        pub code: u32,
        #[prost(uint64, tag = "3")]
        pub n: u64,
    }

    impl crate::Message for Message {
        fn message_id(&self) -> u64 {
            self.id
        }

        fn control_code(&self) -> crate::ProtosocketControlCode {
            crate::ProtosocketControlCode::from_u8(self.code as u8)
        }

        fn set_message_id(&mut self, message_id: u64) {
            self.id = message_id;
        }

        fn cancelled(message_id: u64) -> Self {
            Self {
                id: message_id,
                n: 0,
                code: ProtosocketControlCode::Cancel.as_u8() as u32,
            }
        }

        fn ended(message_id: u64) -> Self {
            Self {
                id: message_id,
                n: 0,
                code: ProtosocketControlCode::End.as_u8() as u32,
            }
        }
    }

    const HANGING_UNARY_MESSAGE: u64 = 2000;
    const HANGING_STREAMING_MESSAGE: u64 = 3000;
    struct TestConnectionService;
    impl std::fmt::Debug for TestConnectionService {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TestConnectionService").finish()
        }
    }

    impl ConnectionService for TestConnectionService {
        type Request = Message;
        type Response = Message;
        // Boxing is used for convenience in tests. You should try to use a static type in your real code.
        type UnaryFutureType = futures::future::BoxFuture<'static, Message>;
        type StreamType = futures::stream::BoxStream<'static, Message>;

        fn new_rpc(
            &mut self,
            request: Self::Request,
        ) -> crate::server::RpcKind<Self::UnaryFutureType, Self::StreamType> {
            if request.id == HANGING_UNARY_MESSAGE {
                RpcKind::Unary(futures::future::pending().boxed())
            } else if request.id == HANGING_STREAMING_MESSAGE {
                RpcKind::Streaming(futures::stream::pending().boxed())
            } else if request.id < 1000 {
                RpcKind::Unary(
                    futures::future::ready(Message {
                        id: request.id,
                        code: ProtosocketControlCode::Normal.as_u8() as u32,
                        n: request.n + 1,
                    })
                    .boxed(),
                )
            } else {
                RpcKind::Streaming(
                    futures::stream::iter((0..request.n).map(move |n| Message {
                        id: request.id,
                        code: ProtosocketControlCode::Normal.as_u8() as u32,
                        n,
                    }))
                    .boxed(),
                )
            }
        }
    }

    pub fn noop_waker() -> Waker {
        const NOOP_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(ptr::null(), &NOOP_WAKER_VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let raw = RawWaker::new(ptr::null(), &NOOP_WAKER_VTABLE);
        // SAFETY: the contracts for RawWaker and RawWakerVTable are trivially upheld by always making new wakers
        unsafe { Waker::from_raw(raw) }
    }

    fn test_server(
        outbound_buffer: usize,
    ) -> (
        mpsc::UnboundedSender<Message>,
        mpsc::Receiver<Message>,
        RpcConnectionServer<TestConnectionService>,
    ) {
        let (inbound_sender, inbound) = mpsc::unbounded_channel();
        let (outbound, outbound_receiver) = mpsc::channel(outbound_buffer);
        let server = RpcConnectionServer::new(TestConnectionService, inbound, outbound);
        (inbound_sender, outbound_receiver, server)
    }

    #[track_caller]
    fn assert_next(
        message: Message,
        outbound_receiver: &mut mpsc::Receiver<Message>,
        context: &mut Context<'_>,
    ) {
        assert_eq!(
            Poll::Ready(Some(message)),
            outbound_receiver.poll_recv(context)
        );
    }

    #[track_caller]
    fn poll_next(
        outbound_receiver: &mut mpsc::Receiver<Message>,
        context: &mut Context<'_>,
    ) -> Message {
        match outbound_receiver.poll_recv(context) {
            Poll::Ready(Some(message)) => message,
            got => panic!("expected message, got {got:?}"),
        }
    }

    #[test]
    fn unary() {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let (inbound_sender, mut outbound_receiver, mut server) = test_server(3);

        // test messages below 1000 are unary. Response is n + 1
        let _ = inbound_sender.send(Message {
            id: 1,
            code: 0,
            n: 1,
        });

        assert_eq!(
            Poll::Pending,
            outbound_receiver.poll_recv(&mut context),
            "nothing should be sent until the server advances to accept the message"
        );

        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        assert_eq!(
            0,
            server.outstanding_unary_rpcs.len(),
            "it completed in one poll"
        );

        assert_next(
            Message {
                id: 1,
                code: 0,
                n: 2,
            },
            &mut outbound_receiver,
            &mut context,
        );
    }

    #[test]
    fn concurrent_unary() {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let (inbound_sender, mut outbound_receiver, mut server) = test_server(3);

        let _ = inbound_sender.send(Message {
            id: 1,
            code: 0,
            n: 1,
        });
        let _ = inbound_sender.send(Message {
            id: 2,
            code: 0,
            n: 3,
        });
        let _ = inbound_sender.send(Message {
            id: 3,
            code: 0,
            n: 5,
        });

        // the server takes up to MAXIMUM_MESSAGES_PER_POLL per poll. I only submitted 3, so they should
        // all get processed in the a single round of poll.
        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        assert_eq!(
            0,
            server.outstanding_unary_rpcs.len(),
            "it completed in one poll"
        );

        let mut concurrent_completions = vec![
            poll_next(&mut outbound_receiver, &mut context),
            poll_next(&mut outbound_receiver, &mut context),
            poll_next(&mut outbound_receiver, &mut context),
        ];
        // they are allowed to complete in any order but I'd like a deterministic order for the assertion
        concurrent_completions.sort();

        assert_eq!(
            vec![
                Message {
                    id: 1,
                    code: 0,
                    n: 2
                },
                Message {
                    id: 2,
                    code: 0,
                    n: 4
                },
                Message {
                    id: 3,
                    code: 0,
                    n: 6
                },
            ],
            concurrent_completions,
        );
        assert_eq!(
            Poll::Pending,
            outbound_receiver.poll_recv(&mut context),
            "no made up messages"
        );
    }

    #[test]
    fn streaming() {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let (inbound_sender, mut outbound_receiver, mut server) = test_server(3);
        // "test" messages at and above 1000 are streaming. Stream has responses n=0..n
        let _ = inbound_sender.send(Message {
            id: 1000,
            code: 0,
            n: 2,
        });
        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );

        let first_message = poll_next(&mut outbound_receiver, &mut context);
        assert_eq!(
            1,
            server.outstanding_streaming_rpcs.len(),
            "there should still be an outstanding rpc because the stream is not done"
        );
        let messages = vec![
            first_message,
            poll_next(&mut outbound_receiver, &mut context),
            poll_next(&mut outbound_receiver, &mut context),
        ];
        // these must come in the correct order.

        assert_eq!(
            vec![
                Message {
                    id: 1000,
                    code: 0,
                    n: 0
                },
                Message {
                    id: 1000,
                    code: 0,
                    n: 1
                },
                Message {
                    id: 1000,
                    code: ProtosocketControlCode::End.as_u8() as u32,
                    n: 0
                },
            ],
            messages,
        );

        assert_eq!(1, server.outstanding_streaming_rpcs.len(), "server has not yet discovered that this rpc is complete. This might change if the poll batch process is changed");
        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        assert_eq!(
            0,
            server.outstanding_streaming_rpcs.len(),
            "all rpcs should be completed"
        );
        assert_eq!(
            Poll::Pending,
            outbound_receiver.poll_recv(&mut context),
            "no made up messages"
        );
    }

    #[test]
    fn streaming_concurrent() {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let (inbound_sender, mut outbound_receiver, mut server) = test_server(3);
        // "test" messages at and above 1000 are streaming. Stream has responses n=0..n
        let _ = inbound_sender.send(Message {
            id: 1000,
            code: 0,
            n: 2,
        });
        let _ = inbound_sender.send(Message {
            id: 1001,
            code: 0,
            n: 2,
        });
        let _ = inbound_sender.send(Message {
            id: 1002,
            code: 0,
            n: 2,
        });

        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        assert_eq!(3, server.outstanding_streaming_rpcs.len());

        let mut messages = vec![
            poll_next(&mut outbound_receiver, &mut context),
            poll_next(&mut outbound_receiver, &mut context),
            poll_next(&mut outbound_receiver, &mut context),
        ];
        assert_eq!(
            Poll::Pending,
            outbound_receiver.poll_recv(&mut context),
            "outbound buffer is only 3. It is unknown if any of the rpcs are complete"
        );
        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        messages.push(poll_next(&mut outbound_receiver, &mut context));
        messages.push(poll_next(&mut outbound_receiver, &mut context));
        messages.push(poll_next(&mut outbound_receiver, &mut context));
        assert_eq!(Poll::Pending, outbound_receiver.poll_recv(&mut context), "though we only defined 6 messages, the server sends an End message for each gracefully ended stream");
        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        messages.push(poll_next(&mut outbound_receiver, &mut context));
        messages.push(poll_next(&mut outbound_receiver, &mut context));
        messages.push(poll_next(&mut outbound_receiver, &mut context));

        // The messages may be intermixed per-rpc, but they must be mutually in order per-rpc.
        // It is a weak assertion to sort these, because that would allow _reordered streams_ to pass the test.
        let first_rpc: Vec<_> = messages
            .iter()
            .filter(|message| message.id == 1000)
            .cloned()
            .collect();
        let second_rpc: Vec<_> = messages
            .iter()
            .filter(|message| message.id == 1001)
            .cloned()
            .collect();
        let third_rpc: Vec<_> = messages
            .iter()
            .filter(|message| message.id == 1002)
            .cloned()
            .collect();

        assert_eq!(
            vec![
                Message {
                    id: 1000,
                    code: 0,
                    n: 0
                },
                Message {
                    id: 1000,
                    code: 0,
                    n: 1
                },
                Message {
                    id: 1000,
                    code: ProtosocketControlCode::End.as_u8() as u32,
                    n: 0
                },
            ],
            first_rpc,
        );
        assert_eq!(
            vec![
                Message {
                    id: 1001,
                    code: 0,
                    n: 0
                },
                Message {
                    id: 1001,
                    code: 0,
                    n: 1
                },
                Message {
                    id: 1001,
                    code: ProtosocketControlCode::End.as_u8() as u32,
                    n: 0
                },
            ],
            second_rpc,
        );
        assert_eq!(
            vec![
                Message {
                    id: 1002,
                    code: 0,
                    n: 0
                },
                Message {
                    id: 1002,
                    code: 0,
                    n: 1
                },
                Message {
                    id: 1002,
                    code: ProtosocketControlCode::End.as_u8() as u32,
                    n: 0
                },
            ],
            third_rpc,
        );
        // server may have 0-3 pending rpcs, but they should all complete with the next poll.
        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        assert_eq!(
            0,
            server.outstanding_streaming_rpcs.len(),
            "all rpcs should be completed"
        );
        assert_eq!(
            Poll::Pending,
            outbound_receiver.poll_recv(&mut context),
            "no made up messages"
        );
    }

    // This test makes sure that the server drops a unary rpc when it asked to do so.
    #[test]
    fn unary_client_cancellation() {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let (inbound_sender, mut outbound_receiver, mut server) = test_server(3);

        let _ = inbound_sender.send(Message {
            id: HANGING_UNARY_MESSAGE,
            code: 0,
            n: 1,
        });
        assert!(pin!(&mut server).poll(&mut context).is_pending());

        assert_eq!(
            1,
            server.outstanding_unary_rpcs.len(),
            "it will never complete"
        );

        let _ = inbound_sender.send(Message {
            id: HANGING_UNARY_MESSAGE,
            code: ProtosocketControlCode::Cancel.as_u8() as u32,
            n: 0,
        });

        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        assert_eq!(
            0,
            server.outstanding_unary_rpcs.len(),
            "all rpcs should be completed"
        );
        assert_eq!(
            Poll::Pending,
            outbound_receiver.poll_recv(&mut context),
            "no made up messages"
        );
    }

    // This test makes sure that the server drops a streaming rpc when it asked to do so.
    #[test]
    fn streaming_client_cancellation() {
        let waker = noop_waker();
        let mut context = Context::from_waker(&waker);

        let (inbound_sender, mut outbound_receiver, mut server) = test_server(3);

        let _ = inbound_sender.send(Message {
            id: HANGING_STREAMING_MESSAGE,
            code: 0,
            n: 1,
        });
        assert!(pin!(&mut server).poll(&mut context).is_pending());

        assert_eq!(
            1,
            server.outstanding_streaming_rpcs.len(),
            "it will never complete"
        );

        let _ = inbound_sender.send(Message {
            id: HANGING_STREAMING_MESSAGE,
            code: ProtosocketControlCode::Cancel.as_u8() as u32,
            n: 0,
        });

        assert!(
            pin!(&mut server).poll(&mut context).is_pending(),
            "server should be pending forever"
        );
        assert_eq!(
            0,
            server.outstanding_streaming_rpcs.len(),
            "all rpcs should be completed"
        );
        assert_eq!(
            Poll::Pending,
            outbound_receiver.poll_recv(&mut context),
            "no made up messages"
        );
    }
}

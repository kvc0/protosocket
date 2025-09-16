use std::{
    collections::VecDeque,
    future::Future,
    io::IoSlice,
    pin::{pin, Pin},
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc,
};

use crate::{
    interrupted,
    types::{ConnectionBindings, DeserializeError, MessageReactor, ReactorStatus},
    would_block, Deserializer, Serializer,
};

/// A bidirectional, message-oriented AsyncRead/AsyncWrite stream wrapper.
///
/// Connections are Futures that you spawn.
/// To send messages, you push them into the outbound message stream.
/// To receive messages, you implement a `MessageReactor`.
///
/// Inbound messages are not wrapped in a Stream, in order to avoid an
/// extra layer of async buffering. If you need to buffer messages or
/// forward them to a Stream, you can do so in the reactor. If you can
/// process them very quickly, you can handle them inline in the reactor
/// callback `on_messages`, which will let you reply as soon as possible.
pub struct Connection<Bindings: ConnectionBindings> {
    stream: Bindings::Stream,
    address: std::net::SocketAddr,
    outbound_messages: mpsc::Receiver<<Bindings::Serializer as Serializer>::Message>,
    outbound_message_buffer: Vec<<Bindings::Serializer as Serializer>::Message>,
    serializer_buffers: Vec<Vec<u8>>,
    send_buffer: VecDeque<Vec<u8>>,
    receive_buffer_unread_index: usize,
    receive_buffer: Vec<u8>,
    max_buffer_length: usize,
    buffer_allocation_increment: usize,
    deserializer: Bindings::Deserializer,
    serializer: Bindings::Serializer,
    reactor: Bindings::Reactor,
}

impl<Bindings: ConnectionBindings> std::fmt::Display for Connection<Bindings> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let read_end = self.receive_buffer_unread_index;
        let read_capacity = self.receive_buffer.len();
        let write_queue = self.send_buffer.len();
        let write_length: usize = self.send_buffer.iter().map(|b| b.len()).sum();
        let address = self.address;
        write!(f, "Connection: {address} {{read{{end: {read_end}, capacity: {read_capacity}}}, write{{queue: {write_queue}, length: {write_length}}}}}")
    }
}

impl<Bindings: ConnectionBindings> Unpin for Connection<Bindings> {}

impl<Bindings: ConnectionBindings> Future for Connection<Bindings> {
    type Output = ();

    /// Take a look at ConnectionBindings for the type definitions used by the Connection
    ///
    /// This method performs the following steps:
    ///
    /// 1. Check for read readiness and read into the receive_buffer (up to max_buffer_length).
    /// 2. Deserialize the read bytes into Messages and store them in the inbound_messages queue.
    /// 3. Dispatch messages as they are deserialized using the user-provided MessageReactor.
    /// 4. Serialize messages from outbound_messages queue, up to max_queued_send_messages.
    /// 5. Send serialized messages.
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // Step 1-3: Receive messages and react to them.
        if self.as_mut().poll_receive(context).is_ready() {
            return Poll::Ready(());
        }

        // Step 4-5: Serialize and send outbound messages
        match self.poll_writev_buffers(context) {
            Ok(false) => {
                log::trace!("write stream is empty or registered for wake when writable");
            }
            Ok(true) => {
                log::debug!("write stream closed");
                return Poll::Ready(());
            }
            Err(e) => {
                log::warn!("error while writing to tcp stream: {e:?}");
                return Poll::Ready(());
            }
        }

        Poll::Pending
    }
}

impl<Lifecycle: ConnectionBindings> Drop for Connection<Lifecycle> {
    fn drop(&mut self) {
        log::debug!("connection dropped")
    }
}

#[derive(Debug)]
enum ReadBufferState {
    /// Done consuming until external liveness is signaled
    Pending,
    /// Need to eagerly wake up again
    MoreToRead,
    /// Connection is disconnected or is to be disconnected
    Disconnected,
    /// Disconnected with an io error
    Error(std::io::Error),
}

impl<Bindings: ConnectionBindings> Connection<Bindings>
where
    <Bindings::Deserializer as Deserializer>::Message: Send,
{
    /// Create a new protosocket Connection with the given stream and reactor.
    ///
    /// Probably you are interested in the `protosocket-server` or `protosocket-prost` crates.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    pub fn new(
        stream: Bindings::Stream,
        address: std::net::SocketAddr,
        deserializer: Bindings::Deserializer,
        serializer: Bindings::Serializer,
        max_buffer_length: usize,
        buffer_allocation_increment: usize,
        max_queued_send_messages: usize,
        outbound_messages: mpsc::Receiver<<Bindings::Serializer as Serializer>::Message>,
        reactor: Bindings::Reactor,
    ) -> Connection<Bindings> {
        // outbound must be queued so it can be called from any context
        Self {
            stream,
            address,
            outbound_messages,
            outbound_message_buffer: Vec::new(),
            send_buffer: Default::default(),
            serializer_buffers: Vec::from_iter((0..max_queued_send_messages).map(|_| Vec::new())),
            receive_buffer: Vec::new(),
            max_buffer_length,
            receive_buffer_unread_index: 0,
            buffer_allocation_increment,
            deserializer,
            serializer,
            reactor,
        }
    }

    /// ensure buffer state and read from the inbound stream
    fn poll_read_inbound(&mut self, context: &mut Context<'_>) -> ReadBufferState {
        if self.receive_buffer.len() < self.max_buffer_length
            && self.receive_buffer.len() - self.receive_buffer_unread_index < self.buffer_allocation_increment
        {
            self.receive_buffer
                .resize(self.receive_buffer.len() + self.buffer_allocation_increment, 0);
        }

        if 0 < self.receive_buffer.len() - self.receive_buffer_unread_index {
            // We can (maybe) read from the connection.
            self.poll_read_from_stream(context)
        } else {
            log::debug!("receive is full {self}");
            ReadBufferState::MoreToRead
        }
    }

    /// process the receive buffer, deserializing bytes into messages
    fn read_inbound_messages_and_react(&mut self) -> ReadBufferState {
        let mut buffer_cursor = 0;
        let state = loop {
            if buffer_cursor == self.receive_buffer_unread_index {
                break ReadBufferState::Pending;
            } else if self.receive_buffer_unread_index < buffer_cursor {
                break ReadBufferState::Error(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "buffer cursor is beyond the end of the receive buffer. Deserializer must not consume more than the buffer length",
                ));
            }

            let buffer = &self.receive_buffer[buffer_cursor..self.receive_buffer_unread_index];
            log::trace!("decode {buffer:?}");
            match self.deserializer.decode(buffer) {
                Ok((length, message)) => {
                    buffer_cursor += length;
                    if self.reactor.on_inbound_messages([message]) == ReactorStatus::Disconnect {
                        log::debug!("reactor requested disconnect");
                        return ReadBufferState::Disconnected;
                    }
                }
                Err(e) => match e {
                    DeserializeError::IncompleteBuffer { next_message_size } => {
                        if self.max_buffer_length < next_message_size {
                            log::error!("tried to receive message that is too long. Resetting connection - max: {}, requested: {}", self.max_buffer_length, next_message_size);
                            return ReadBufferState::Disconnected;
                        }
                        log::debug!("waiting for the next message of length {next_message_size}");
                        break ReadBufferState::Pending;
                    }
                    DeserializeError::InvalidBuffer => {
                        log::error!("message was invalid - broken stream");
                        return ReadBufferState::Disconnected;
                    }
                    DeserializeError::SkipMessage { distance } => {
                        if self.receive_buffer_unread_index - buffer_cursor < distance {
                            log::trace!("cannot skip yet, need to read more. Skipping: {distance}, remaining:{}", self.receive_buffer_unread_index - buffer_cursor);
                            break ReadBufferState::Pending;
                        }
                        log::debug!("skipping message of length {distance}");
                        buffer_cursor += distance;
                    }
                },
            }
        };
        if buffer_cursor != 0 && buffer_cursor == self.receive_buffer_unread_index {
            log::trace!("read buffer complete - resetting: {self}");
            self.receive_buffer_unread_index = 0;
        } else if buffer_cursor != 0 {
            log::trace!("read buffer partially consumed - shifting: {self}");
            self.receive_buffer
                .copy_within(buffer_cursor..self.receive_buffer_unread_index, 0);
            self.receive_buffer_unread_index -= buffer_cursor;
        }
        state
    }

    /// read from the TcpStream
    fn poll_read_from_stream(&mut self, context: &mut Context<'_>) -> ReadBufferState {
        let mut buffer = ReadBuf::new(&mut self.receive_buffer[self.receive_buffer_unread_index..]);
        match pin!(&mut self.stream).poll_read(context, &mut buffer) {
            Poll::Ready(Ok(_)) => {
                let distance = buffer.filled().len();
                if distance == 0 {
                    log::debug!("read 0 bytes, stream is closed");
                    ReadBufferState::Disconnected
                } else {
                    self.receive_buffer_unread_index += distance;
                    log::trace!(
                        "read from stream: {distance}b, total: {}b",
                        self.receive_buffer_unread_index
                    );
                    ReadBufferState::MoreToRead
                }
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Poll::Ready(Err(ref err)) if would_block(err) => {
                log::trace!("read everything. No longer readable");
                ReadBufferState::Pending
            }
            Poll::Ready(Err(ref err)) if interrupted(err) => {
                log::trace!("interrupted, so try again later");
                ReadBufferState::MoreToRead
            }
            Poll::Ready(Err(err)) => {
                log::warn!("error while reading from tcp stream: {err:?}");
                ReadBufferState::Error(err)
            }
            Poll::Pending => ReadBufferState::Pending,
        }
    }

    /// This serializes work-in-progress messages and moves them over into the write queue
    fn poll_serialize_outbound_messages(&mut self, context: &mut Context<'_>) -> Poll<()> {
        let max_outbound = self.serializer_buffers.len();
        if max_outbound == 0 {
            log::debug!("send is full: {self}");
            // pending on a network status event
            return Poll::Pending;
        }

        let start_len = self.send_buffer.len();
        for _ in 0..max_outbound {
            let message = match self.outbound_message_buffer.pop() {
                Some(next) => next,
                None => {
                    match self.outbound_messages.poll_recv_many(
                        context,
                        &mut self.outbound_message_buffer,
                        self.serializer_buffers.capacity(),
                    ) {
                        Poll::Ready(count) => {
                            // ugh, I know. but poll_recv_many is much cheaper than poll_recv,
                            // and poll_recv requires &mut Vec. Otherwise this would be a VecDeque with no reverse.
                            self.outbound_message_buffer.reverse();
                            match self.outbound_message_buffer.pop() {
                                Some(next) => next,
                                None => {
                                    assert_eq!(0, count);
                                    log::info!("outbound message channel was closed");
                                    return Poll::Ready(());
                                }
                            }
                        }
                        Poll::Pending => {
                            log::trace!(
                                "no more messages to serialize, and we are pending for more"
                            );
                            break;
                        }
                    }
                }
            };
            let mut buffer = self
                .serializer_buffers
                .pop()
                .expect("max_outbound is limited by serializer_buffers length");
            self.serializer.encode(message, &mut buffer);
            if self.max_buffer_length < buffer.len() {
                log::error!(
                    "tried to send too large a message. Max {}, attempted: {}",
                    self.max_buffer_length,
                    buffer.len()
                );
                return Poll::Ready(());
            }
            log::trace!(
                "serialized message and enqueueing outbound buffer: {}b",
                buffer.len()
            );
            // queue up a writev
            self.send_buffer.push_back(buffer);
        }
        let new_len = self.send_buffer.len();
        if start_len != new_len {
            log::debug!(
                "serialized {} messages, waking task to look for more input",
                new_len - start_len
            );
        }
        // This portion of poll is either pending for more messages, or it is the network's turn to be pending.
        // If the network is ready, it will push buffers and re-notify serialization.
        Poll::Pending
    }

    /// Send buffers to the tcp stream, and recycle them if they are fully written
    fn poll_writev_buffers(
        &mut self,
        context: &mut Context<'_>,
    ) -> std::result::Result<bool, std::io::Error> {
        /// I need to figure out how to get this from the os rather than hardcoding. 16 is the lowest I've seen mention of,
        /// and I've seen 1024 more commonly.
        const UIO_MAXIOV: usize = 128;

        loop {
            if self.poll_serialize_outbound_messages(context).is_ready() {
                log::debug!("outbound channel closed");
                return Ok(true);
            }
            break if self.send_buffer.is_empty() {
                log::trace!("send buffer is empty");
                Ok(false)
            } else {
                let buffers: Vec<IoSlice> = self
                    .send_buffer
                    .iter()
                    .take(UIO_MAXIOV)
                    .map(|v| IoSlice::new(v))
                    .collect();
                match pin!(&mut self.stream).poll_write_vectored(context, &buffers) {
                    Poll::Pending => {
                        log::debug!("writev not ready - waiting for wake");
                        Ok(false)
                    }
                    Poll::Ready(Ok(0)) => {
                        log::info!("write stream was closed");
                        Ok(true)
                    }
                    Poll::Ready(Ok(written)) => {
                        self.rotate_send_buffers(written);
                        // we need to go around again to make sure we're either done writing or pending
                        continue;
                    }
                    // Would block "errors" are the OS's way of saying that the
                    // connection is not actually ready to perform this I/O operation.
                    Poll::Ready(Err(ref err)) if would_block(err) => {
                        log::trace!("would block - no longer writable");
                        continue;
                    }
                    Poll::Ready(Err(ref err)) if interrupted(err) => {
                        log::trace!("write interrupted - try again later");
                        continue;
                    }
                    // other errors terminate the stream
                    Poll::Ready(Err(err)) => {
                        log::warn!(
                            "error while writing to tcp stream: {err:?}, buffers: {}, {}b: {:?}",
                            buffers.len(),
                            buffers.iter().map(|b| b.len()).sum::<usize>(),
                            buffers.into_iter().map(|b| b.len()).collect::<Vec<_>>()
                        );
                        Err(err)
                    }
                }
            };
        }
    }

    /// Discard all written bytes, and recycle the buffers that are fully written
    fn rotate_send_buffers(&mut self, mut written: usize) {
        let total_written = written;
        while 0 < written {
            if let Some(mut front) = self.send_buffer.pop_front() {
                if front.len() <= written {
                    written -= front.len();
                    log::trace!(
                        "recycling buffer of length {}, remaining: {}",
                        front.len(),
                        written
                    );

                    // Reuse the buffer!
                    front.clear();
                    self.serializer_buffers.push(front);
                } else {
                    // Walk the buffer forward through a replacement. It will still amortize the allocation,
                    // but this is not optimal. It's relatively easier to manage though, and I'm a busy person.
                    log::debug!("after writing {total_written}b, shifting partially written buffer of {}b by {written}b", front.len());
                    let replacement = front[written..].to_vec();
                    self.send_buffer.push_front(replacement);
                    break;
                }
            } else {
                log::error!("rotated all buffers but {written} bytes unaccounted for");
                break;
            }
        }
    }

    fn poll_receive(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        loop {
            match self.poll_read_inbound(context) {
                ReadBufferState::Pending => {
                    log::debug!("consumed all that I can from the read stream for now {self}");
                    return Poll::Pending;
                }
                ReadBufferState::MoreToRead => {
                    log::debug!("more to read");
                    self.read_inbound_messages_and_react();
                    continue;
                }
                ReadBufferState::Disconnected => {
                    log::info!("read connection closed");
                    return Poll::Ready(());
                }
                ReadBufferState::Error(e) => {
                    log::warn!("error while reading from tcp stream: {e:?}");
                    return Poll::Ready(());
                }
            }
        }
    }
}

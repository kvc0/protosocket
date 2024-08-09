use std::{
    collections::VecDeque,
    future::Future,
    io::IoSlice,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::sync::mpsc;

use crate::{
    interrupted,
    types::{ConnectionBindings, DeserializeError, MessageReactor, ReactorStatus},
    would_block, Deserializer, Serializer,
};

/// A bidirectional, message-oriented tcp stream wrapper.
///
/// Connections are Futures that you spawn.
/// To send messages, you push them into the outbound message stream.
/// To receive messages, you implement a `MessageReactor`. Inbound messages are not
/// wrapped in a Stream, in order to avoid an extra layer of async buffering. If you
/// need to buffer messages or forward them to a Stream, you can do so in the reactor.
pub struct Connection<Bindings: ConnectionBindings> {
    stream: tokio::net::TcpStream,
    address: std::net::SocketAddr,
    outbound_messages: mpsc::Receiver<<Bindings::Serializer as Serializer>::Message>,
    outbound_message_buffer: Vec<<Bindings::Serializer as Serializer>::Message>,
    inbound_messages: VecDeque<<Bindings::Deserializer as Deserializer>::Message>,
    serializer_buffers: Vec<Vec<u8>>,
    send_buffer: VecDeque<Vec<u8>>,
    receive_buffer_slice_end: usize,
    receive_buffer_start_offset: usize,
    receive_buffer: Vec<u8>,
    receive_buffer_swap: Vec<u8>,
    max_buffer_length: usize,
    max_queued_send_messages: usize,
    deserializer: Bindings::Deserializer,
    serializer: Bindings::Serializer,
    reactor: Bindings::Reactor,
}

impl<Bindings: ConnectionBindings> std::fmt::Display for Connection<Bindings> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let read_start = self.receive_buffer_start_offset;
        let read_end = self.receive_buffer_slice_end;
        let write_queue = self.send_buffer.len();
        let write_length: usize = self.send_buffer.iter().map(|b| b.len()).sum();
        let address = self.address;
        write!(f, "Connection: {address} {{read{{start: {read_start}, end: {read_end}}}, write{{queue: {write_queue}, length: {write_length}}}}}")
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
    /// 3. Process all messages in the inbound queue using the user-provided MessageReactor.
    /// 4. Serialize messages from outbound_messages queue, up to max_queued_send_messages.
    /// 5. Check for write readiness and send serialized messages.
    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.receive_buffer_slice_end < self.max_buffer_length {
            // Step 1: Check if there's space in the receive buffer and the stream is ready for reading
            match self.stream.poll_read_ready(context) {
                Poll::Ready(status) => {
                    if let Err(e) = status {
                        log::error!("error while polling read readiness: {e:?}");
                        return Poll::Ready(());
                    }
                    // Not looping on receive, so if I'm readable I'll just come back around
                    context.waker().wake_by_ref();

                    // Step 1a: read raw bytes from the stream
                    match self.read_inbound() {
                        Ok(true) => {
                            log::info!("read connection closed");
                            return Poll::Ready(());
                        }
                        Ok(false) => {
                            log::trace!("read connection is still open");
                        }
                        Err(e) => {
                            log::warn!("error while reading from tcp stream: {e:?}");
                            return Poll::Ready(());
                        }
                    }
                }
                Poll::Pending => {
                    log::trace!("read side is up to date");
                }
            }
        } else {
            log::trace!("receive buffer is full");
        }

        // Step 2: Deserialize read bytes into messages
        match self.read_inbound_messages_into_read_queue() {
            Ok(true) => {
                log::info!("read queue closed");
                return Poll::Ready(());
            }
            Ok(false) => {
                log::trace!("read queue is still open");
            }
            Err(e) => {
                log::warn!("error while reading from buffer: {e:?}");
                return Poll::Ready(());
            }
        }
        {
            // Create a new scope to allow for for fine-grained borrowing of Self.
            // Step 3: Process inbound messages with the Connection's MessageReactor
            let Self {
                reactor,
                inbound_messages,
                ..
            } = &mut *self;
            if reactor.on_inbound_messages(inbound_messages.drain(..)) == ReactorStatus::Disconnect
            {
                log::debug!("reactor requested disconnect");
                return Poll::Ready(());
            }
        }

        // Step 4: Prepare outbound messages for sending. We serialize the outbound bytes here as per the
        // Connection's serializer
        if let Poll::Ready(early_out) = self.poll_serialize_outbound_messages(context) {
            return Poll::Ready(early_out);
        }

        // Step 5: Send outbound messages if the stream is ready for writing
        if !self.send_buffer.is_empty() {
            match self.stream.poll_write_ready(context) {
                Poll::Ready(status) => {
                    if let Err(e) = status {
                        log::error!("error while polling write readiness: {e:?}");
                        return Poll::Ready(());
                    }

                    // Step 5a: write raw bytes to the stream
                    match self.writev_buffers() {
                        Ok(true) => {
                            log::info!("write connection closed");
                            return Poll::Ready(());
                        }
                        Ok(false) => {
                            log::trace!("write connection is still open");
                        }
                        Err(e) => {
                            log::warn!("error while writing to tcp stream: {e:?}");
                            return Poll::Ready(());
                        }
                    }

                    if !self.outbound_message_buffer.is_empty() {
                        // not looping on send, so if I still have stuff to send I'll come back around
                        context.waker().wake_by_ref();
                    }
                }
                Poll::Pending => {
                    log::trace!("write side is too slow");
                }
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

impl<Bindings: ConnectionBindings> Connection<Bindings>
where
    <Bindings::Deserializer as Deserializer>::Message: Send,
{
    /// Create a new protosocket Connection with the given stream and reactor.
    ///
    /// Probably you are interested in the `protosocket-server` or `protosocket-prost` crates.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    pub fn new(
        stream: tokio::net::TcpStream,
        address: std::net::SocketAddr,
        deserializer: Bindings::Deserializer,
        serializer: Bindings::Serializer,
        max_buffer_length: usize,
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
            inbound_messages: VecDeque::with_capacity(max_queued_send_messages),
            send_buffer: Default::default(),
            serializer_buffers: Vec::from_iter((0..1).map(|_| Vec::new())),
            receive_buffer: Vec::new(),
            receive_buffer_swap: Vec::new(),
            max_buffer_length,
            receive_buffer_start_offset: 0,
            max_queued_send_messages,
            receive_buffer_slice_end: 0,
            deserializer,
            serializer,
            reactor,
        }
    }

    /// Ok(true) when the remote end closed the connection
    /// Ok(false) when it's done and the remote is not closed
    /// Err should probably be fatal
    fn read_inbound(&mut self) -> std::result::Result<bool, std::io::Error> {
        const BUFFER_INCREMENT: usize = 16 * (2 << 10);
        if self.receive_buffer.len() - self.receive_buffer_slice_end < BUFFER_INCREMENT {
            self.receive_buffer.resize(
                (self.receive_buffer.len() + BUFFER_INCREMENT).min(self.max_buffer_length),
                0,
            );
        }

        if 0 < self.receive_buffer.len() - self.receive_buffer_slice_end {
            // We can (maybe) read from the connection.
            match self.read_from_stream() {
                Ok(closed) => {
                    if closed {
                        log::info!("read connection closed");
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
                Err(e) => {
                    log::trace!("io error during read: {e:?}");
                    Err(e)
                }
            }
        } else {
            log::debug!("receive is full {self}");
            Ok(false)
        }
    }

    /// process the receive buffer, deserializing bytes into messages
    /// * Ok(false) if deserialization worked and the connection is still open (happy case).
    /// * Ok(true) if the was a fatal deserialization error and the connection should close.
    /// * Err if there was an io error (probably fatal to the connection)
    fn read_inbound_messages_into_read_queue(&mut self) -> Result<bool, std::io::Error> {
        while self.receive_buffer_start_offset < self.receive_buffer_slice_end {
            if self.inbound_messages.capacity() == self.inbound_messages.len() {
                // can't accept any more inbound messages right now
                log::debug!("inbound message queue is draining too slowly");
                break;
            }

            let buffer = &self.receive_buffer
                [self.receive_buffer_start_offset..self.receive_buffer_slice_end];
            match self.deserializer.decode(buffer) {
                Ok((length, message)) => {
                    self.receive_buffer_start_offset += length;
                    self.inbound_messages.push_back(message);
                }
                Err(e) => match e {
                    DeserializeError::IncompleteBuffer { next_message_size } => {
                        if self.max_buffer_length < next_message_size {
                            log::error!("tried to receive message that is too long. Resetting connection - max: {}, requested: {}", self.max_buffer_length, next_message_size);
                            return Ok(true);
                        }
                        if self.max_buffer_length
                            < self.receive_buffer_slice_end + next_message_size
                        {
                            let length =
                                self.receive_buffer_slice_end - self.receive_buffer_start_offset;
                            log::debug!(
                                "rotating {}b of buffer to make room for next message {}b",
                                length,
                                next_message_size
                            );
                            self.receive_buffer_swap.clear();
                            self.receive_buffer_swap.extend_from_slice(
                                &self.receive_buffer[self.receive_buffer_start_offset
                                    ..self.receive_buffer_slice_end],
                            );
                            std::mem::swap(&mut self.receive_buffer, &mut self.receive_buffer_swap);
                            self.receive_buffer_start_offset = 0;
                            self.receive_buffer_slice_end = length;
                        }
                        log::debug!("waiting for the next message of length {next_message_size}");
                        break;
                    }
                    DeserializeError::InvalidBuffer => {
                        log::error!("message was invalid - broken stream");
                        return Ok(true);
                    }
                    DeserializeError::SkipMessage { distance } => {
                        if self.receive_buffer_slice_end - self.receive_buffer_start_offset
                            < distance
                        {
                            log::trace!("cannot skip yet, need to read more. Skipping: {distance}, remaining:{}", self.receive_buffer_slice_end - self.receive_buffer_start_offset);
                            break;
                        }
                        log::debug!("skipping message of length {distance}");
                        self.receive_buffer_start_offset += distance;
                    }
                },
            }
        }
        if self.receive_buffer_start_offset == self.receive_buffer_slice_end
            && self.receive_buffer_start_offset != 0
        {
            log::debug!("read buffer complete - resetting");
            self.receive_buffer_start_offset = 0;
            self.receive_buffer_slice_end = 0;
        }
        Ok(false)
    }

    /// read from the TcpStream
    /// * Ok(false) if the read worked and the socket is still open (happy case).
    /// * Ok(true) if the read side of the socket is closed.
    /// * Err if there was an io error (probably fatal to the connection)
    fn read_from_stream(&mut self) -> Result<bool, std::io::Error> {
        match self
            .stream
            .try_read(&mut self.receive_buffer[self.receive_buffer_slice_end..])
        {
            Ok(0) => {
                log::info!(
                    "connection was shut down as recv returned 0. Requested {}",
                    self.receive_buffer.len() - self.receive_buffer_slice_end
                );
                return Ok(true);
            }
            Ok(bytes_read) => {
                self.receive_buffer_slice_end += bytes_read;
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {
                log::trace!("read everything. No longer readable");
            }
            Err(ref err) if interrupted(err) => {
                log::trace!("interrupted, so try again later");
            }
            // Other errors we'll consider fatal.
            Err(err) => {
                log::warn!("error while reading from tcp stream. buffer length: {}b, offset: {}, offered length {}b, err: {err:?}", self.receive_buffer.len(), self.receive_buffer_slice_end, self.receive_buffer.len() - self.receive_buffer_slice_end);
                return Err(err);
            }
        }
        Ok(false)
    }

    fn room_in_send_buffer(&self) -> usize {
        self.max_queued_send_messages - self.send_buffer.len()
    }

    /// This serializes work-in-progress messages and moves them over into the write queue
    fn poll_serialize_outbound_messages(&mut self, context: &mut Context<'_>) -> Poll<()> {
        let max_outbound = self.room_in_send_buffer();
        if max_outbound == 0 {
            log::debug!("send is full: {self}");
            // pending on a network status event
            return Poll::Pending;
        }

        for _ in 0..max_outbound {
            let message = match self.outbound_message_buffer.pop() {
                Some(next) => next,
                None => {
                    match self.outbound_messages.poll_recv_many(
                        context,
                        &mut self.outbound_message_buffer,
                        self.max_queued_send_messages,
                    ) {
                        Poll::Ready(count) => match self.outbound_message_buffer.pop() {
                            Some(next) => next,
                            None => {
                                assert_eq!(0, count);
                                log::info!("outbound message channel was closed");
                                return Poll::Ready(());
                            }
                        },
                        Poll::Pending => {
                            log::trace!("no messages to serialize");
                            break;
                        }
                    }
                }
            };
            let mut buffer = self.serializer_buffers.pop().unwrap_or_default();
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
        Poll::Pending
    }

    /// Send buffers to the tcp stream, and recycle them if they are fully written
    fn writev_buffers(&mut self) -> std::result::Result<bool, std::io::Error> {
        if self.send_buffer.is_empty() {
            return Ok(false);
        }

        /// I need to figure out how to get this from the os rather than hardcoding. 16 is the lowest I've seen mention of,
        /// and I've seen 1024 more commonly.
        const UIO_MAXIOV: usize = 128;

        let buffers: Vec<IoSlice> = self
            .send_buffer
            .iter()
            .take(UIO_MAXIOV)
            .map(|v| IoSlice::new(v))
            .collect();
        match self.stream.try_write_vectored(&buffers) {
            Ok(0) => {
                log::info!("write stream was closed");
                return Ok(true);
            }
            Ok(written) => {
                self.rotate_send_buffers(written);
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {
                log::trace!("would block - no longer writable");
            }
            Err(ref err) if interrupted(err) => {
                log::trace!("write interrupted - try again later");
            }
            // other errors terminate the stream
            Err(err) => {
                log::warn!(
                    "error while writing to tcp stream: {err:?}, buffers: {}, {}b: {:?}",
                    buffers.len(),
                    buffers.iter().map(|b| b.len()).sum::<usize>(),
                    buffers.into_iter().map(|b| b.len()).collect::<Vec<_>>()
                );
                return Err(err);
            }
        }
        Ok(false)
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
                    // SAFETY: This is purely a buffer, and u8 does not require drop.
                    unsafe { front.set_len(0) };
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
}

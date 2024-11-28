use std::{
    future::Future,
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
    inbound_messages: Vec<<Bindings::Deserializer as Deserializer>::Message>,
    send_buffer: Vec<u8>,
    receive_buffer_slice_end: usize,
    receive_buffer_start_offset: usize,
    receive_buffer: Vec<u8>,
    receive_buffer_swap: Vec<u8>,
    max_buffer_length: usize,
    deserializer: Bindings::Deserializer,
    serializer: Bindings::Serializer,
    reactor: Bindings::Reactor,
}

impl<Bindings: ConnectionBindings> std::fmt::Display for Connection<Bindings> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let read_start = self.receive_buffer_start_offset;
        let read_end = self.receive_buffer_slice_end;
        let read_capacity = self.receive_buffer.len();
        let write_queue = self.send_buffer.len();
        let address = self.address;
        write!(f, "Connection: {address} {{read{{start: {read_start}, end: {read_end}, capacity: {read_capacity}}}, write{{queue: {write_queue}}}}}")
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
        // Step 1: Check if there's space in the receive buffer and the stream is ready for reading
        if let Some(early_out) = self.as_mut().poll_receive(context) {
            return early_out;
        }

        // Step 2: Deserialize read bytes into messages
        if let Some(early_out) = self.as_mut().poll_deserialize(context) {
            return early_out;
        }

        // Step 3: Process inbound messages with the Connection's MessageReactor
        if let Some(early_out) = self.poll_react() {
            return early_out;
        }

        // Step 4: Prepare outbound messages for sending. We serialize the outbound bytes here as per the
        // Connection's serializer
        if let Poll::Ready(early_out) = self.poll_serialize_outbound_messages(context) {
            return Poll::Ready(early_out);
        }

        // Step 5: Send outbound messages if the stream is ready for writing
        if let Some(early_out) = self.poll_outbound(context) {
            return early_out;
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
    WaitingForMore,
    /// Need to eagerly wake up again
    PartiallyConsumed,
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
            inbound_messages: Vec::with_capacity(max_queued_send_messages),
            send_buffer: Vec::with_capacity(max_buffer_length),
            receive_buffer: Vec::new(),
            receive_buffer_swap: Vec::new(),
            max_buffer_length,
            receive_buffer_start_offset: 0,
            receive_buffer_slice_end: 0,
            deserializer,
            serializer,
            reactor,
        }
    }

    /// ensure buffer state and read from the inbound stream
    fn read_inbound(&mut self) -> ReadBufferState {
        const BUFFER_INCREMENT: usize = 2 << 20;
        if self.receive_buffer.len() < self.max_buffer_length
            && self.receive_buffer.len() - self.receive_buffer_slice_end < BUFFER_INCREMENT
        {
            self.receive_buffer.reserve(BUFFER_INCREMENT);
            // SAFETY: This is a buffer, and u8 is not read until after the read syscall returns. Read initializes the buffer values.
            //         I reserved the additional space above, so the additional space is valid.
            // This was done because resizing the buffer shows up on heat maps.
            #[allow(clippy::uninit_vec)]
            unsafe {
                self.receive_buffer
                    .set_len(self.receive_buffer.len() + BUFFER_INCREMENT)
            };
        }

        if 0 < self.receive_buffer.len() - self.receive_buffer_slice_end {
            // We can (maybe) read from the connection.
            self.read_from_stream()
        } else {
            log::debug!("receive is full {self}");
            ReadBufferState::WaitingForMore
        }
    }

    /// process the receive buffer, deserializing bytes into messages
    fn read_inbound_messages_into_read_queue(&mut self) -> ReadBufferState {
        let state = loop {
            if self.receive_buffer_start_offset == self.receive_buffer_slice_end {
                break ReadBufferState::WaitingForMore;
            }
            if self.inbound_messages.capacity() == self.inbound_messages.len() {
                // can't accept any more inbound messages right now
                log::debug!("full batch of messages read from the socket");
                break ReadBufferState::PartiallyConsumed;
            }

            let buffer = &self.receive_buffer
                [self.receive_buffer_start_offset..self.receive_buffer_slice_end];
            match self.deserializer.decode(buffer) {
                Ok((length, message)) => {
                    self.receive_buffer_start_offset += length;
                    self.inbound_messages.push(message);
                }
                Err(e) => match e {
                    DeserializeError::IncompleteBuffer { next_message_size } => {
                        if self.max_buffer_length < next_message_size {
                            log::error!("tried to receive message that is too long. Resetting connection - max: {}, requested: {}", self.max_buffer_length, next_message_size);
                            return ReadBufferState::Disconnected;
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
                        break ReadBufferState::WaitingForMore;
                    }
                    DeserializeError::InvalidBuffer => {
                        log::error!("message was invalid - broken stream");
                        return ReadBufferState::Disconnected;
                    }
                    DeserializeError::SkipMessage { distance } => {
                        if self.receive_buffer_slice_end - self.receive_buffer_start_offset
                            < distance
                        {
                            log::trace!("cannot skip yet, need to read more. Skipping: {distance}, remaining:{}", self.receive_buffer_slice_end - self.receive_buffer_start_offset);
                            break ReadBufferState::WaitingForMore;
                        }
                        log::debug!("skipping message of length {distance}");
                        self.receive_buffer_start_offset += distance;
                    }
                },
            }
        };
        if self.receive_buffer_start_offset == self.receive_buffer_slice_end
            && self.receive_buffer_start_offset != 0
        {
            log::debug!("read buffer complete - resetting: {self}");
            self.receive_buffer_start_offset = 0;
            self.receive_buffer_slice_end = 0;
        }
        state
    }

    /// read from the TcpStream
    fn read_from_stream(&mut self) -> ReadBufferState {
        match self
            .stream
            .try_read(&mut self.receive_buffer[self.receive_buffer_slice_end..])
        {
            Ok(0) => {
                log::info!(
                    "connection was shut down as recv returned 0. Requested {}",
                    self.receive_buffer.len() - self.receive_buffer_slice_end
                );
                ReadBufferState::Disconnected
            }
            Ok(bytes_read) => {
                self.receive_buffer_slice_end += bytes_read;
                ReadBufferState::PartiallyConsumed
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {
                log::trace!("read everything. No longer readable");
                ReadBufferState::WaitingForMore
            }
            Err(ref err) if interrupted(err) => {
                log::trace!("interrupted, so try again later");
                ReadBufferState::PartiallyConsumed
            }
            // Other errors we'll consider fatal.
            Err(err) => {
                log::warn!("error while reading from tcp stream. buffer length: {}b, offset: {}, offered length {}b, err: {err:?}", self.receive_buffer.len(), self.receive_buffer_slice_end, self.receive_buffer.len() - self.receive_buffer_slice_end);
                ReadBufferState::Error(err)
            }
        }
    }

    fn room_in_send_buffer(&self) -> usize {
        self.max_buffer_length
            .saturating_sub(self.send_buffer.len())
    }

    /// This serializes work-in-progress messages and moves them over into the write queue
    fn poll_serialize_outbound_messages(&mut self, context: &mut Context<'_>) -> Poll<()> {
        let room_bytes = self.room_in_send_buffer();
        if room_bytes == 0 {
            log::debug!("send is full: {self}");
            // pending on a network status event
            return Poll::Pending;
        }

        let start_len = self.send_buffer.len();
        while self.send_buffer.len() < self.max_buffer_length {
            let message = match self.outbound_message_buffer.pop() {
                Some(next) => next,
                None => {
                    match self.outbound_messages.poll_recv_many(
                        context,
                        &mut self.outbound_message_buffer,
                        32,
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
                            log::trace!("no messages to serialize");
                            break;
                        }
                    }
                }
            };
            let start = self.send_buffer.len();
            self.serializer.encode(message, &mut self.send_buffer);
            let size = self.send_buffer.len() - start;
            if self.max_buffer_length < size {
                log::error!(
                    "tried to send too large a message. Max {}, attempted: {}",
                    self.max_buffer_length,
                    size,
                );
                return Poll::Ready(());
            }
            log::trace!("serialized {size}b message and enqueueing outbound buffer: {self}");
        }
        let new_len = self.send_buffer.len();
        if start_len != new_len {
            log::debug!(
                "serialized {}b, waking task to look for more input",
                new_len - start_len
            );
            // if the serializer made progress, there may be more work that the network or outbound channel can do.
            // make sure the task gets another round to try.
            context.waker().wake_by_ref();
        }
        Poll::Pending
    }

    /// Send buffer to the tcp stream
    fn write_buffer(&mut self) -> std::result::Result<bool, std::io::Error> {
        match self.stream.try_write(&self.send_buffer) {
            Ok(0) => {
                log::info!("write stream was closed");
                return Ok(true);
            }
            Ok(written) => {
                self.rotate_send_buffer(written);
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
                log::warn!("error while writing to tcp stream: {err:?} {self}");
                return Err(err);
            }
        }
        Ok(false)
    }

    /// Discard all written bytes, and recycle the buffers that are fully written
    fn rotate_send_buffer(&mut self, written: usize) {
        if self.send_buffer.len() <= written {
            log::trace!(
                "recycling send buffer buffer of length {}",
                self.send_buffer.len()
            );

            // Reuse the buffer!
            // SAFETY: This is purely a buffer, and u8 does not require drop.
            unsafe { self.send_buffer.set_len(0) };
        } else {
            // Walk the buffer forward through a replacement.
            // This will need to be replaced with an offset and eager reuse like the receive buffer has. It's a little simpler for send though.
            let retain = self.send_buffer.len() - written;
            log::debug!("after writing {written}b, shifting remaining buffer of {retain}b");
            unsafe {
                self.send_buffer.set_len(retain);
                #[allow(clippy::ptr_offset_with_cast)]
                core::intrinsics::copy(
                    self.send_buffer.as_ptr().offset(written as isize),
                    self.send_buffer.as_mut_ptr(),
                    retain,
                );
            }
        }
    }

    fn poll_receive(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Option<Poll<()>> {
        loop {
            break if self.receive_buffer_slice_end < self.max_buffer_length {
                match self.stream.poll_read_ready(context) {
                    Poll::Ready(status) => {
                        if let Err(e) = status {
                            log::error!("error while polling read readiness: {e:?}");
                            return Some(Poll::Ready(()));
                        }

                        // Step 1a: read raw bytes from the stream
                        match self.read_inbound() {
                            ReadBufferState::WaitingForMore => {
                                log::debug!(
                                    "consumed all that I can from the read stream for now {self}"
                                );
                                continue;
                            }
                            ReadBufferState::PartiallyConsumed => {
                                log::debug!("more to read");
                                continue;
                            }
                            ReadBufferState::Disconnected => {
                                log::info!("read connection closed");
                                return Some(Poll::Ready(()));
                            }
                            ReadBufferState::Error(e) => {
                                log::warn!("error while reading from tcp stream: {e:?}");
                                return Some(Poll::Ready(()));
                            }
                        }
                    }
                    Poll::Pending => {
                        log::trace!("read side is up to date");
                    }
                }
            } else {
                log::debug!("receive buffer is full");
            };
        }
        None
    }

    fn poll_deserialize(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Option<Poll<()>> {
        if self.receive_buffer_start_offset == self.receive_buffer_slice_end {
            // Only when the read buffer is empty should deserialization skip waking the task.
            // I always want the task to be pending on network readability, network writability, or the outbound message channel.
            // Otherwise, I want the task to be woken.
            return None;
        }
        let start_len = self.inbound_messages.len();
        match self.read_inbound_messages_into_read_queue() {
            ReadBufferState::WaitingForMore => {
                log::trace!("read queue is still open");
                let new_len = self.inbound_messages.len();
                if start_len != new_len {
                    // If you don't wake here, then when the read side gets full, you won't be registered for network
                    // activity wakes.
                    // You can skip this wake when you are deserializing a large message, because that is waiting
                    // for more inbound data to arrive. You are only WaitingForMore when you're mid-message,
                    // and if you've just got the one message you can wait for the network.
                    // There's no deserializer progress to report if you didn't deserialize anything.
                    context.waker().wake_by_ref();
                }
            }
            ReadBufferState::PartiallyConsumed => {
                log::debug!("read buffer partially consumed for responsiveness {self}");
                context.waker().wake_by_ref();
            }
            ReadBufferState::Disconnected => {
                log::info!("read queue closed");
                return Some(Poll::Ready(()));
            }
            ReadBufferState::Error(e) => {
                log::warn!("error while reading from buffer: {e:?}");
                return Some(Poll::Ready(()));
            }
        }
        None
    }

    fn poll_react(&mut self) -> Option<Poll<()>> {
        let Self {
            reactor,
            inbound_messages,
            ..
        } = &mut *self;
        if reactor.on_inbound_messages(inbound_messages.drain(..)) == ReactorStatus::Disconnect {
            log::debug!("reactor requested disconnect");
            return Some(Poll::Ready(()));
        }
        None
    }

    fn poll_outbound(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Option<Poll<()>> {
        loop {
            break if !self.send_buffer.is_empty() {
                // the write half of the stream is only considered for readiness when there is something to write.
                match self.stream.poll_write_ready(context) {
                    Poll::Ready(status) => {
                        if let Err(e) = status {
                            log::error!("error while polling write readiness: {e:?}");
                            return Some(Poll::Ready(()));
                        }

                        // Step 5a: write raw bytes to the stream
                        log::debug!("writing {self}");
                        match self.write_buffer() {
                            Ok(true) => {
                                log::info!("write connection closed");
                                return Some(Poll::Ready(()));
                            }
                            Ok(false) => {
                                log::debug!("wrote {self}");
                            }
                            Err(e) => {
                                log::warn!("error while writing to tcp stream: {e:?}");
                                return Some(Poll::Ready(()));
                            }
                        }

                        log::trace!(
                            "wrote output, checking for more and possibly registering for wake"
                        );
                        continue;
                    }
                    Poll::Pending => {
                        log::debug!("waiting for outbound wake");
                    }
                }
            } else {
                log::trace!("nothing to send");
            };
        }
        None
    }
}

use std::{
    collections::VecDeque,
    io::{IoSlice, Read, Write},
    task::{Context, Poll},
};

use mio::{net::TcpStream, Registry};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::{
    interrupted,
    types::{ConnectionBindings, DeserializeError},
    would_block, Deserializer, Serializer,
};

pub struct Connection<Bindings: ConnectionBindings> {
    stream: mio::net::TcpStream,
    outbound_messages: mpsc::Receiver<<Bindings::Serializer as Serializer>::Message>,
    inbound_messages: PollSender<<Bindings::Deserializer as Deserializer>::Message>,
    serializer_buffers: Vec<Vec<u8>>,
    send_buffer: VecDeque<Vec<u8>>,
    receive_buffer_slice_end: usize,
    receive_buffer_start_offset: usize,
    receive_buffer: Vec<u8>,
    receive_buffer_swap: Vec<u8>,
    max_message_length: usize,
    max_queued_send_messages: usize,
    deserializer: Bindings::Deserializer,
    serializer: Bindings::Serializer,
    readable: ReadState,
    writable: bool,
}

impl<Bindings: ConnectionBindings> std::fmt::Display for Connection<Bindings> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let read_state = self.readable;
        let read_start = self.receive_buffer_start_offset;
        let read_end = self.receive_buffer_slice_end;
        let write_state = self.writable;
        let write_queue = self.send_buffer.len();
        let write_length: usize = self.send_buffer.iter().map(|b| b.len()).sum();
        write!(f, "Connection: {{read{{state: {read_state}, start: {read_start}, end: {read_end}}}, write{{writable: {write_state}, queue: {write_queue}, length: {write_length}}}}}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadState {
    NotReadable,
    Readable,
    Closed,
}

impl std::fmt::Display for ReadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ReadState::NotReadable => "not_readable",
                ReadState::Readable => "readable",
                ReadState::Closed => "closed",
            }
        )
    }
}

impl<Lifecycle: ConnectionBindings> Drop for Connection<Lifecycle> {
    fn drop(&mut self) {
        log::debug!("connection dropped")
    }
}

#[derive(Debug, Clone, Copy)]
pub enum NetworkStatusEvent {
    Readable,
    Writable,
    ReadableWritable,
}

impl TryFrom<&mio::event::Event> for NetworkStatusEvent {
    type Error = ();

    fn try_from(value: &mio::event::Event) -> Result<Self, ()> {
        match (value.is_readable(), value.is_writable()) {
            (true, true) => Ok(NetworkStatusEvent::ReadableWritable),
            (true, false) => Ok(NetworkStatusEvent::Readable),
            (false, true) => Ok(NetworkStatusEvent::Writable),
            (false, false) => Err(()),
        }
    }
}

impl<Bindings: ConnectionBindings> Connection<Bindings>
where
    <Bindings::Deserializer as Deserializer>::Message: Send,
{
    #[allow(clippy::type_complexity)]
    pub fn new(
        stream: TcpStream,
        deserializer: Bindings::Deserializer,
        serializer: Bindings::Serializer,
        max_message_length: usize,
        max_queued_send_messages: usize,
    ) -> (
        mpsc::Sender<<Bindings::Serializer as Serializer>::Message>,
        mpsc::Receiver<<Bindings::Deserializer as Deserializer>::Message>,
        Self,
    ) {
        let (outbound_sender, outbound_receiver) = mpsc::channel(256);
        let (inbound_sender, inbound_receiver) = mpsc::channel(256);
        (
            outbound_sender,
            inbound_receiver,
            Self {
                stream,
                outbound_messages: outbound_receiver,
                inbound_messages: PollSender::new(inbound_sender),
                send_buffer: Default::default(),
                serializer_buffers: Vec::from_iter((0..1).map(|_| Vec::new())),
                receive_buffer: Vec::new(),
                receive_buffer_swap: Vec::new(),
                max_message_length,
                receive_buffer_start_offset: 0,
                max_queued_send_messages,
                receive_buffer_slice_end: 0,
                deserializer,
                serializer,
                readable: ReadState::NotReadable,
                writable: false,
            },
        )
    }

    /// Drop self and gracefully deregister
    pub fn deregister(mut self, registry: &Registry) {
        match registry.deregister(&mut self.stream) {
            Ok(_) => (),
            Err(e) => {
                log::warn!("failed to deregister stream from registry: {e:?}");
            }
        }
    }

    /// to make sure you stay live you want to handle_mio_connection_events before you work on read/write buffers
    pub fn handle_connection_event(&mut self, event: NetworkStatusEvent) {
        match event {
            NetworkStatusEvent::Readable => {
                log::trace!("connection is readable");
                self.readable = ReadState::Readable;
            }
            NetworkStatusEvent::Writable => {
                log::trace!("connection is writable");
                self.writable = true;
            }
            NetworkStatusEvent::ReadableWritable => {
                // &mio::event::Event
                log::trace!("connection is rw");
                self.writable = true;
                self.readable = ReadState::Readable;
            }
        }
    }

    /// true when the connection has stuff to do. This can return true when the inbound half of the connection is closed (e.g., nc)
    pub fn has_work_in_flight(&self) -> bool {
        !(self.send_buffer.is_empty() && self.receive_buffer_slice_end == 0)
    }

    /// to make sure you stay live you want to handle_mio_connection_events before you work on read/write buffers
    ///
    /// Ok(true) when the remote end closed the connection
    /// Ok(false) when it's done and the remote is not closed
    /// Err should probably be fatal
    pub fn poll_read_inbound(&mut self) -> std::result::Result<bool, std::io::Error> {
        match self.readable {
            ReadState::NotReadable => return Ok(false),
            ReadState::Closed => return Ok(true),
            ReadState::Readable => {
                // fall through
            }
        }

        if self.receive_buffer_slice_end == self.max_message_length {
            log::trace!("receive buffer is full");
        } else {
            const BUFFER_INCREMENT: usize = 16 * (2 << 10);
            if self.receive_buffer.len() - self.receive_buffer_slice_end < BUFFER_INCREMENT {
                self.receive_buffer.resize(
                    (self.receive_buffer.len() + BUFFER_INCREMENT).min(self.max_message_length),
                    0,
                );
            }

            if 0 < self.receive_buffer.len() - self.receive_buffer_slice_end {
                // We can (maybe) read from the connection.
                if let Some(early_out) = self.read_from_stream() {
                    return early_out;
                }
            } else {
                log::debug!("receive is full {self}");
            }
        }
        Ok(false)
    }

    pub fn dispatch_messages_from_read_queue(
        &mut self,
        context: &mut Context,
    ) -> Result<bool, std::io::Error> {
        while self.receive_buffer_start_offset < self.receive_buffer_slice_end {
            match self.inbound_messages.poll_reserve(context) {
                std::task::Poll::Ready(Ok(_)) => {
                    // ready
                }
                std::task::Poll::Ready(Err(_e)) => {
                    log::info!("inbound channel closed - closing connection");
                    self.readable = ReadState::Closed;
                    return Ok(true);
                }
                std::task::Poll::Pending => {
                    // can't accept any more inbound messages right now
                    log::debug!("inbound message queue is draining too slowly");
                    break;
                }
            }

            let buffer = &self.receive_buffer
                [self.receive_buffer_start_offset..self.receive_buffer_slice_end];
            match self.deserializer.decode(buffer) {
                Ok((length, message)) => {
                    self.receive_buffer_start_offset += length;
                    if let Err(_e) = self.inbound_messages.send_item(message) {
                        log::warn!("failure to send to inbound channel after reserving space - closing channel");
                        self.readable = ReadState::Closed;
                        return Ok(true);
                    }
                }
                Err(e) => match e {
                    DeserializeError::IncompleteBuffer { next_message_size } => {
                        if self.max_message_length < next_message_size {
                            log::error!("tried to receive message that is too long. Resetting connection - max: {}, requested: {}", self.max_message_length, next_message_size);
                            self.readable = ReadState::Closed;
                            return Ok(true);
                        }
                        if self.max_message_length
                            < self.receive_buffer_slice_end + next_message_size
                        {
                            let length =
                                self.receive_buffer_slice_end - self.receive_buffer_start_offset;
                            log::info!(
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
                        self.readable = ReadState::Closed;
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
        if self.receive_buffer_start_offset == self.receive_buffer_slice_end {
            log::debug!("read buffer complete - resetting");
            self.receive_buffer_start_offset = 0;
            self.receive_buffer_slice_end = 0;
        }
        Ok(false)
    }

    fn read_from_stream(&mut self) -> Option<Result<bool, std::io::Error>> {
        match self
            .stream
            .read(&mut self.receive_buffer[self.receive_buffer_slice_end..])
        {
            Ok(0) => {
                log::info!(
                    "connection was shut down as recv returned 0. Requested {}",
                    self.receive_buffer.len() - self.receive_buffer_slice_end
                );
                self.readable = ReadState::Closed;
                return Some(Ok(true));
            }
            Ok(bytes_read) => {
                self.receive_buffer_slice_end += bytes_read;
            }
            // Would block "errors" are the OS's way of saying that the
            // connection is not actually ready to perform this I/O operation.
            Err(ref err) if would_block(err) => {
                log::trace!("read everything. No longer readable");
                self.readable = ReadState::NotReadable;
            }
            Err(ref err) if interrupted(err) => {
                log::trace!("interrupted, so try again later");
            }
            // Other errors we'll consider fatal.
            Err(err) => {
                log::warn!("error while reading from tcp stream. buffer length: {}b, offset: {}, offered length {}b, err: {err:?}", self.receive_buffer.len(), self.receive_buffer_slice_end, self.receive_buffer.len() - self.receive_buffer_slice_end);
                return Some(Err(err));
            }
        }
        None
    }

    /// This serializes work-in-progress messages and moves them over into the write queue
    pub fn poll_serialize_oubound(&mut self, context: &mut Context<'_>) -> Poll<()> {
        let max_outbound = self.max_queued_send_messages - self.send_buffer.len();
        if max_outbound == 0 {
            log::debug!("send is full: {self}");
            context.waker().wake_by_ref();
            return Poll::Pending;
        }
        let mut outbound = Vec::with_capacity(max_outbound.min(32));
        match self
            .outbound_messages
            .poll_recv_many(context, &mut outbound, max_outbound)
        {
            std::task::Poll::Ready(how_many) => {
                log::trace!("polled {how_many} messages to send");
                for message in outbound.drain(..) {
                    let mut buffer = self.serializer_buffers.pop().unwrap_or_default();
                    self.serializer.encode(message, &mut buffer);
                    if self.max_message_length < buffer.len() {
                        log::error!(
                            "tried to send too large a message. Max {}, attempted: {}",
                            self.max_message_length,
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
                // look for more messages
                context.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Pending => Poll::Pending,
        }
    }

    /// to make sure you stay live you want to handle_mio_connection_events before you work on read/write buffers
    pub fn poll_write_buffers(&mut self) -> std::result::Result<bool, std::io::Error> {
        if !self.writable || self.send_buffer.is_empty() {
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
        match self.stream.write_vectored(&buffers) {
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
                self.writable = false;
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

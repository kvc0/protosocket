use std::{
    cmp::min,
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
    receive_buffer_length: usize,
    receive_buffer: Vec<u8>,
    deserializer: Bindings::Deserializer,
    serializer: Bindings::Serializer,
    readable: ReadState,
    writable: bool,
}

#[derive(Debug)]
enum ReadState {
    NotReadable,
    Readable,
    Closed,
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
    ) -> (
        mpsc::Sender<<Bindings::Serializer as Serializer>::Message>,
        mpsc::Receiver<<Bindings::Deserializer as Deserializer>::Message>,
        Self,
    ) {
        let (outbound_sender, outbound_receiver) = mpsc::channel(128);
        let (inbound_sender, inbound_receiver) = mpsc::channel(128);
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
                receive_buffer_length: 0,
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
        !(self.send_buffer.is_empty() && self.receive_buffer_length == 0)
    }

    /// to make sure you stay live you want to handle_mio_connection_events before you work on read/write buffers
    ///
    /// Ok(true) when the remote end closed the connection
    /// Ok(false) when it's done and the remote is not closed
    /// Err should probably be fatal
    pub fn poll_read_inbound(
        &mut self,
        context: &mut Context<'_>,
    ) -> std::result::Result<bool, std::io::Error> {
        match self.readable {
            ReadState::NotReadable => return Ok(false),
            ReadState::Closed => return Ok(true),
            ReadState::Readable => {
                // fall through
            }
        }

        const BUFFER_INCREMENT: usize = 16 * (2 << 10);
        if self.receive_buffer.len() - self.receive_buffer_length < BUFFER_INCREMENT {
            self.receive_buffer.resize(
                (self.receive_buffer.len() / 4).clamp(BUFFER_INCREMENT, 2 << 20),
                0,
            );
        }

        // We can (maybe) read from the connection.
        match self
            .stream
            .read(&mut self.receive_buffer[self.receive_buffer_length..])
        {
            Ok(0) => {
                log::debug!("connection was shut down as recv returned 0");
                self.readable = ReadState::Closed;
                return Ok(true);
            }
            Ok(bytes_read) => {
                self.receive_buffer_length += bytes_read;

                let mut consumed = 0;

                while consumed < self.receive_buffer_length {
                    match self.inbound_messages.poll_reserve(context) {
                        std::task::Poll::Ready(Ok(_)) => {
                            // ready
                        }
                        std::task::Poll::Ready(Err(_e)) => {
                            log::debug!("inbound channel closed - closing connection");
                            self.readable = ReadState::Closed;
                            return Ok(true);
                        }
                        std::task::Poll::Pending => {
                            // can't accept any more inbound messages right now
                            break;
                        }
                    }

                    let buffer = &self.receive_buffer[consumed..self.receive_buffer_length];
                    match self.deserializer.decode(buffer) {
                        Ok((length, message)) => {
                            consumed += length;
                            if let Err(_e) = self.inbound_messages.send_item(message) {
                                log::warn!("failure to send to inbound channel after reserving space - closing channel");
                                self.readable = ReadState::Closed;
                                return Ok(true);
                            }
                        }
                        Err(e) => {
                            match e {
                                DeserializeError::IncompleteBuffer { next_message_size } => {
                                    log::debug!("waiting for the next message of length {next_message_size}");
                                    break;
                                }
                                DeserializeError::InvalidBuffer => {
                                    log::debug!("message was invalid");
                                    consumed = self.receive_buffer_length;
                                }
                                DeserializeError::SkipMessage { distance } => {
                                    log::debug!("skipping message of length {distance}");
                                    consumed = min(self.receive_buffer_length, consumed + distance);
                                }
                            }
                        }
                    }
                }
                self.receive_buffer_length -= consumed;
                if consumed != 0 && self.receive_buffer_length != 0 {
                    // partial read - have to shift the buffer because I haven't made it smarter.
                    // at least I'm pulling multiple messages out if there's a bunch of little messages in a big buffer...
                    log::debug!("shifting buffer by {consumed} bytes");
                    self.receive_buffer.rotate_left(consumed);
                }
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
                log::debug!("error while reading from tcp stream: {err:?}");
                return Err(err);
            }
        }
        Ok(false)
    }

    /// This serializes work-in-progress messages and moves them over into the write queue
    pub fn poll_serialize_oubound(&mut self, context: &mut Context<'_>) -> Poll<()> {
        let mut outbound = Vec::with_capacity(16);
        match self
            .outbound_messages
            .poll_recv_many(context, &mut outbound, 16)
        {
            std::task::Poll::Ready(how_many) => {
                log::trace!("polled {how_many} messages to send");
                for message in outbound.drain(..) {
                    let mut buffer = self.serializer_buffers.pop().unwrap_or_default();
                    self.serializer.encode(message, &mut buffer);
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
    pub fn poll_write_buffers(&mut self) -> std::result::Result<(), std::io::Error> {
        if !self.writable || self.send_buffer.is_empty() {
            return Ok(());
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
                log::debug!("error while writing to tcp stream: {err:?}, buffers: {buffers:?}");
                return Err(err);
            }
        }
        Ok(())
    }

    fn rotate_send_buffers(&mut self, mut written: usize) {
        while 0 < written {
            if let Some(mut front) = self.send_buffer.pop_front() {
                if front.len() <= written {
                    written -= front.len();
                    log::trace!(
                        "rotated buffer of length {}, remaining: {}",
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
                    log::debug!("shifting serialized buffer by {written} bytes");
                    let replacement = front[written..].to_vec();
                    self.send_buffer.push_front(replacement);
                    break;
                }
            } else {
                log::debug!("rotated buffers. {} remaining", self.send_buffer.len());
                break;
            }
        }
    }
}

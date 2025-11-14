use crate::{DeserializeError, Serialize};

/// An encoder takes messages and produces outbound bytes.
pub trait Encoder {
    /// The message type consumed by this serializer.
    type Message;

    /// The type this serializer produces.
    ///
    /// If you want to write to raw vectors, consider wrapping your serializer
    /// with [PooledEncoder] and using that instead.
    type Serialized: bytes::Buf;

    /// Encode a message into a buffer.
    fn encode(&mut self, message: Self::Message) -> Self::Serialized;

    /// Buffers are sent back to the encoder once the message is sent.
    /// Buffers are not guaranteed to be advanced to the end.
    /// You can reset and reuse your buffer, if appropriate.
    fn return_buffer(&mut self, _buffer: Self::Serialized) {
        // drop by default
    }
}

/// A decoder takes inbound bytes and produces messages.
pub trait Decoder {
    /// The message type produced by this deserializer.
    type Message;

    /// Decode a message from the buffer, or tell why you can't.
    ///
    /// You must not consume more bytes than the message you produce.
    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError>;
}

impl<T> Encoder for T
where
    T: Serialize,
{
    type Message = T::Message;
    type Serialized = OwnedBuffer;

    fn encode(&mut self, message: Self::Message) -> Self::Serialized {
        let mut buffer = Vec::new();
        self.serialize_into_buffer(message, &mut buffer);
        OwnedBuffer::new(buffer)
    }
}

/// A basic Buf wrapper for a byte array. This works for simple apis, but if you
/// have latency, memory, or cpu constraints, you should be using PooledEncoder
/// or another more sophisticated memory reuse mechanism.
pub struct OwnedBuffer {
    buffer: Vec<u8>,
    cursor: usize,
}
impl OwnedBuffer {
    fn new(buffer: Vec<u8>) -> Self {
        Self { buffer, cursor: 0 }
    }
}
impl bytes::Buf for OwnedBuffer {
    #[inline(always)]
    fn remaining(&self) -> usize {
        self.buffer.len() - self.cursor
    }

    #[inline(always)]
    fn chunk(&self) -> &[u8] {
        &self.buffer[self.cursor..]
    }

    #[inline(always)]
    fn advance(&mut self, cnt: usize) {
        assert!(self.buffer.len() <= self.cursor + cnt);
        self.cursor += cnt;
    }
}

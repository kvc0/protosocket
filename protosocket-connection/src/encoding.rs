use crate::{DeserializeError, Serialize};

/// A codec combines an encoder and decoder for a message type.
///
/// You can make a codec via a tuple of `(Encoder, Decoder)` or
/// by implementing one yourself.
///
/// ## Buffer lifecycles
/// The lifecycle of a message, with respect to encoding and decoding, is:
///
/// ### 1 decode
/// `Decoder::decode()` produces a Message from inbound bytes.
/// If you want to reuse allocations, you can pull from your own pool in here.
///
/// ### 2 your application process
/// If you want to reuse the decoder memory later, you will either need to use
/// a smart pointer on Drop, or you will need to carry the buffer into your
/// logical response message type. This way, it can be delivered back to the
/// Codec via `Encoder::encode()` later.
///
/// ### 3 encode
/// `Encoder::encode()` produces outbound bytes from a logical Message. This is
/// where you can return decoder buffers to your pool, if you carried them through.
///
/// ### 4 buffer return
/// After the encoded bytes are sent, `Encoder::return_buffer()` is called with
/// the serialized buffer. You can reuse or drop it as appropriate.
///
/// ## About pooling
/// The Encoder's encode->return_buffer cycle is independent of the Decoder's lifecycle.
/// Pooling for the Encoder is easy, and you should totally do it. `protosocket`
/// provides a `PooledEncoder` wrapper and a `Serialize` trait you can use for the easiest
/// way to pool your outbound buffer allocations.
///
/// Decoder pooling is more application-specific, and depends very much on your encoding and
/// application types for its usefulness, so you will need to implement it yourself.
pub trait Codec: Encoder + Decoder {}

impl<E, D> Codec for (E, D)
where
    E: Encoder,
    D: Decoder,
{
}
impl<E, D> Encoder for (E, D)
where
    E: Encoder,
    D: Decoder,
{
    type Message = E::Message;
    type Serialized = E::Serialized;
    fn encode(&mut self, message: Self::Message) -> Self::Serialized {
        self.0.encode(message)
    }

    fn return_buffer(&mut self, buffer: Self::Serialized) {
        self.0.return_buffer(buffer);
    }
}
impl<E, D> Decoder for (E, D)
where
    E: Encoder,
    D: Decoder,
{
    type Message = D::Message;
    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError> {
        self.1.decode(buffer)
    }
}

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

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip_all, name = "raw_serialize")
    )]
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

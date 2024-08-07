/// A serializer takes messages and produces outbound bytes.
pub trait Serializer: Unpin + Send {
    /// The message type consumed by this serializer.
    type Message: Send;

    /// Encode a message into a buffer.
    fn encode(&mut self, response: Self::Message, buffer: &mut impl bytes::BufMut);
}

/// A deserializer takes inbound bytes and produces messages.
pub trait Deserializer: Unpin + Send {
    /// The message type produced by this deserializer.
    type Message: Send;

    /// Decode a message from the buffer, or tell why you can't.
    ///
    /// You must not consume more bytes than the message you produce.
    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError>;
}

/// Errors that can occur when deserializing a message.
#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    /// Buffer will be retained and you will be called again later with more bytes
    #[error("Need more bytes to decode the next message")]
    IncompleteBuffer {
        /// This is a hint to the connection for how many more bytes should be read.
        /// You may be called again before you get another buffer with at least this
        /// many bytes.
        next_message_size: usize,
    },
    /// Buffer will be discarded
    #[error("Bad buffer")]
    InvalidBuffer,
    /// distance will be skipped
    #[error("Skip message")]
    SkipMessage {
        /// If a message is not to be serviced, you can skip it. This is how many
        /// bytes will be skipped.
        /// You may be called again before this message is skipped and you may need
        /// to repeat the skip.
        distance: usize,
    },
}

/// A message reactor is a stateful object that processes inbound messages.
/// You receive &mut self, and you receive your messages by value.
///
/// A message reactor may be a server which spawns a task per message, or a client which
/// matches response ids to a HashMap of concurrent requests with oneshot completions.
///
/// Your message reactor and your tcp connection share their fate - when one drops or
/// disconnects, the other does too.
pub trait MessageReactor: Unpin + Send + 'static {
    type Inbound;

    /// Called from the connection's driver task when messages are received.
    ///
    /// You must take all of the messages quickly: Blocking here will block the connection.
    /// If you can't accept new messages and you can't queue, you should consider returning
    /// Disconnect.
    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus;
}

/// What the connection should do after processing a batch of inbound messages.
#[derive(Debug, PartialEq, Eq)]
pub enum ReactorStatus {
    /// Continue processing messages.
    Continue,
    /// Disconnect the tcp connection.
    Disconnect,
}

/// Define the types for a Connection.
///
/// A protosocket uses only 1 kind of message per port. This is a constraint to keep types
/// straightforward. If you want multiple message types, you should consider using protocol
/// buffers `oneof` fields. You would use a wrapper type to hold the oneof and any additional
/// metadata, like a request ID or trace id.
pub trait ConnectionBindings: 'static {
    /// The deserializer for this connection.
    type Deserializer: Deserializer;
    /// The serializer for this connection.
    type Serializer: Serializer;
    /// The message reactor for this connection.
    type Reactor: MessageReactor<Inbound = <Self::Deserializer as Deserializer>::Message>;
}

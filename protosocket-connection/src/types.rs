pub trait Serializer: Unpin + Send {
    type Message: Send;

    fn encode(&mut self, response: Self::Message, buffer: &mut impl bytes::BufMut);
}

pub trait Deserializer: Unpin + Send {
    type Message: Send;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    /// Buffer will be retained and you will be called again later with more bytes
    #[error("Need more bytes to decode the next message")]
    IncompleteBuffer { next_message_size: usize },
    /// Buffer will be discarded
    #[error("Bad buffer")]
    InvalidBuffer,
    /// distance will be skipped
    #[error("Skip message")]
    SkipMessage { distance: usize },
}

pub trait MessageReactor: Unpin + Send + 'static {
    type Inbound;

    /// you must take all of the messages quickly. If you need to disconnect, return err.
    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus;
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReactorStatus {
    Continue,
    Disconnect,
}

pub trait ConnectionBindings: 'static {
    type Deserializer: Deserializer;
    type Serializer: Serializer;
    type Reactor: MessageReactor<Inbound = <Self::Deserializer as Deserializer>::Message>;
}

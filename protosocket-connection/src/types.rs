pub trait Serializer: Unpin {
    type Message: Send;

    fn encode(&mut self, response: Self::Message, buffer: &mut impl bytes::BufMut);
}

pub trait Deserializer: Unpin {
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

pub trait ConnectionBindings {
    type Deserializer: Deserializer;
    type Serializer: Serializer;
}

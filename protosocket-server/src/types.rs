use std::future::Future;

pub trait Serializer: Unpin {
    type Response;

    fn encode(&mut self, response: Self::Response, buffer: &mut impl bytes::BufMut);
}

pub trait Deserializer: Unpin {
    type Request;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Request), DeserializeError>;
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

pub trait ConnectionLifecycle: Unpin + Sized {
    type ServerState: Unpin;
    type Deserializer: Deserializer;
    type Serializer: Serializer;
    type MessageFuture: Future<Output = <Self::Serializer as Serializer>::Response>;

    /// A new connection lifecycle starts here. If you have a state machine, initialize it here. This is your constructor.
    fn on_connect(server_state: &Self::ServerState)
        -> (Self, Self::Deserializer, Self::Serializer);

    fn on_message(
        &mut self,
        message: <Self::Deserializer as Deserializer>::Request,
    ) -> Self::MessageFuture;
}

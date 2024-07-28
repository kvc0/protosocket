use std::{future::Future, marker::PhantomData};

use protocolsocket_server::{ConnectionLifecycle, DeserializeError, Deserializer, Serializer};

pub struct ProtocolBufferSocket<State, Request, Response, MessageFuture> {
    server_state: State,
    _phantom: PhantomData<(Request, Response, MessageFuture)>,
}

pub trait MessageExecutor<Message, MessageFuture> {
    fn execute(&mut self, message: Message) -> MessageFuture;
}

impl<State, Request, Response, MessageFuture>
    ProtocolBufferSocket<State, Request, Response, MessageFuture>
{
    /// Access the handle for server state
    pub fn server_state(&mut self) -> &mut State {
        &mut self.server_state
    }
}

impl<State, Request, Response, MessageFuture> ConnectionLifecycle
    for ProtocolBufferSocket<State, Request, Response, MessageFuture>
where
    State: MessageExecutor<Request, MessageFuture> + Clone + Unpin,
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
    MessageFuture: Future<Output = Response> + Unpin,
{
    type ServerState = State;
    type Deserializer = ProstSerializer<Request, Response>;
    type Serializer = ProstSerializer<Request, Response>;
    type MessageFuture = MessageFuture;

    fn on_connect(
        server_state: &Self::ServerState,
    ) -> (Self, Self::Deserializer, Self::Serializer) {
        (
            Self {
                server_state: server_state.clone(),
                _phantom: PhantomData,
            },
            ProstSerializer {
                _phantom: PhantomData,
            },
            ProstSerializer {
                _phantom: PhantomData,
            },
        )
    }

    fn on_message(
        &mut self,
        message: <Self::Deserializer as Deserializer>::Request,
    ) -> Self::MessageFuture {
        self.server_state.execute(message)
    }
}

pub struct ProstSerializer<Request, Response> {
    _phantom: PhantomData<(Request, Response)>,
}

impl<Request, Response> Serializer for ProstSerializer<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
{
    type Response = Response;

    fn encode(&mut self, response: Self::Response, buffer: &mut impl bytes::BufMut) {
        match response.encode_length_delimited(buffer) {
            Ok(_) => {
                log::trace!("encoded reply");
            }
            Err(e) => {
                log::error!("encoding error: {e:?}");
            }
        }
    }
}
impl<Request, Response> Deserializer for ProstSerializer<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
{
    type Request = Request;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Request), protocolsocket_server::DeserializeError> {
        match prost::decode_length_delimiter(buffer.chunk()) {
            Ok(length) => {
                log::trace!("reading {length} bytes from buffer");
                match <Self::Request as prost::Message>::decode_length_delimited(buffer) {
                    Ok(message) => {
                        log::trace!("decoded request");
                        Ok((0, message))
                    }
                    Err(e) => {
                        log::debug!("could not decode message: {e:?}");
                        Err(DeserializeError::InvalidMessage)
                    }
                }
            }
            Err(e) => {
                log::debug!("could not decode message length: {e:?}");
                Err(DeserializeError::InvalidMessage)
            }
        }
    }
}

use std::{future::Future, marker::PhantomData};

use protosocket_connection::{ConnectionLifecycle, Deserializer};

use crate::{prost_serializer::ProstSerializer, MessageExecutor};

pub struct ProtocolBufferSocket<State, Request, Response, MessageFuture> {
    server_state: State,
    _phantom: PhantomData<(Request, Response, MessageFuture)>,
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

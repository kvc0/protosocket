use std::marker::PhantomData;

use protosocket_connection::ConnectionBindings;

use crate::prost_serializer::ProstSerializer;

pub struct ProtocolBufferConnectionBindings<Request, Response> {
    _phantom: PhantomData<(Request, Response)>,
}

impl<Request, Response> ConnectionBindings
    for ProtocolBufferConnectionBindings<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
{
    type Deserializer = ProstSerializer<Request, Response>;
    type Serializer = ProstSerializer<Request, Response>;
}

use std::marker::PhantomData;

use protosocket_connection::ConnectionBindings;

use crate::prost_serializer::ProstSerializer;

pub struct ProstServerConnectionBindings<Request, Response> {
    _phantom: PhantomData<(Request, Response)>,
}

impl<Request, Response> ConnectionBindings
    for ProstServerConnectionBindings<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
{
    type Deserializer = ProstSerializer<Request, Response>;
    type Serializer = ProstSerializer<Request, Response>;
}

pub struct ProstClientConnectionBindings<Request, Response> {
    _phantom: PhantomData<(Request, Response)>,
}

impl<Request, Response> ConnectionBindings
    for ProstClientConnectionBindings<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Default + Unpin,
{
    type Deserializer = ProstSerializer<Response, Request>;
    type Serializer = ProstSerializer<Response, Request>;
}

use std::marker::PhantomData;

use protosocket::{ConnectionBindings, MessageReactor};

use crate::prost_serializer::ProstSerializer;

/// A convenience type for binding a `ProstSerializer` to a server-side
/// `protosocket::Connection`.
pub struct ProstServerConnectionBindings<Request, Response, Reactor> {
    _phantom: PhantomData<(Request, Response, Reactor)>,
}

impl<Request, Response, Reactor> ConnectionBindings
    for ProstServerConnectionBindings<Request, Response, Reactor>
where
    Request: prost::Message + Default + Unpin + 'static,
    Response: prost::Message + Unpin + 'static,
    Reactor: MessageReactor<Inbound = Request>,
{
    type Deserializer = ProstSerializer<Request, Response>;
    type Serializer = ProstSerializer<Request, Response>;
    type Reactor = Reactor;
}

/// A convenience type for binding a `ProstSerializer` to a client-side
/// `protosocket::Connection`.
pub struct ProstClientConnectionBindings<Request, Response, Reactor> {
    _phantom: PhantomData<(Request, Response, Reactor)>,
}

impl<Request, Response, Reactor> ConnectionBindings
    for ProstClientConnectionBindings<Request, Response, Reactor>
where
    Request: prost::Message + Default + Unpin + 'static,
    Response: prost::Message + Default + Unpin + 'static,
    Reactor: MessageReactor<Inbound = Response>,
{
    type Deserializer = ProstSerializer<Response, Request>;
    type Serializer = ProstSerializer<Response, Request>;
    type Reactor = Reactor;
}

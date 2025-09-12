use std::marker::PhantomData;

use protosocket::{ConnectionBindings, MessageReactor};

use crate::prost_serializer::ProstSerializer;

/// A convenience type for binding a `ProstSerializer` to a server-side
/// `protosocket::Connection`.
pub struct ProstServerConnectionBindings<Request, Response, Reactor, Stream> {
    _phantom: PhantomData<(Request, Response, Reactor, Stream)>,
}

impl<Request, Response, Reactor, TStream> ConnectionBindings
    for ProstServerConnectionBindings<Request, Response, Reactor, TStream>
where
    Request: prost::Message + Default + Unpin + 'static,
    Response: prost::Message + Unpin + 'static,
    Reactor: MessageReactor<Inbound = Request>,
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    type Deserializer = ProstSerializer<Request, Response>;
    type Serializer = ProstSerializer<Request, Response>;
    type Reactor = Reactor;
    type Stream = TStream;
}

/// A convenience type for binding a `ProstSerializer` to a client-side
/// `protosocket::Connection`.
pub struct ProstClientConnectionBindings<Request, Response, Reactor, TStream> {
    _phantom: PhantomData<(Request, Response, Reactor, TStream)>,
}

impl<Request, Response, Reactor, TStream> ConnectionBindings
    for ProstClientConnectionBindings<Request, Response, Reactor, TStream>
where
    Request: prost::Message + Default + Unpin + 'static,
    Response: prost::Message + Default + Unpin + 'static,
    Reactor: MessageReactor<Inbound = Response>,
    TStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    type Deserializer = ProstSerializer<Response, Request>;
    type Serializer = ProstSerializer<Response, Request>;
    type Reactor = Reactor;
    type Stream = TStream;
}

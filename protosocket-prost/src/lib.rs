mod prost_serializer;
mod protocolbuffer_socket;

pub use protocolbuffer_socket::ProtocolBufferConnectionBindings;

/// Service task is called here per message; you can choose to directly execute or spawn.
pub trait MessageExecutor<Message, MessageFuture> {
    fn execute(&mut self, message: Message) -> MessageFuture;
}

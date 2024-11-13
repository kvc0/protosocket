use core::panic;
use std::{
    collections::hash_map::Entry,
    marker::PhantomData,
    sync::{atomic::AtomicBool, Arc},
};

use protosocket::{ConnectionBindings, MessageReactor, ReactorStatus};

use crate::{
    message::ProtosocketControlCode,
    reactor::completion_registry::{Completion, CompletionRegistry, RpcRegistrar},
    Message,
};

pub struct RpcCompletionConnectionBindings<Serializer, Deserializer>(
    PhantomData<(Serializer, Deserializer)>,
);
impl<Serializer, Deserializer> ConnectionBindings
    for RpcCompletionConnectionBindings<Serializer, Deserializer>
where
    Serializer: protosocket::Serializer + 'static,
    Serializer::Message: Message,
    Deserializer: protosocket::Deserializer + 'static,
    Deserializer::Message: Message,
{
    type Deserializer = Deserializer;
    type Serializer = Serializer;
    type Reactor =
        RpcCompletionReactor<Deserializer::Message, DoNothingMessageHandler<Deserializer::Message>>;
}

#[derive(Debug)]
pub struct RpcCompletionReactor<Inbound, TUnregisteredMessageHandler>
where
    Inbound: Message,
    TUnregisteredMessageHandler: UnregisteredMessageHandler<Inbound = Inbound>,
{
    rpc_registry: CompletionRegistry<Inbound>,
    is_alive: Arc<AtomicBool>,
    unregistered_message_handler: TUnregisteredMessageHandler,
}
impl<Inbound, TUnregisteredMessageHandler>
    RpcCompletionReactor<Inbound, TUnregisteredMessageHandler>
where
    Inbound: Message,
    TUnregisteredMessageHandler: UnregisteredMessageHandler<Inbound = Inbound>,
{
    #[allow(clippy::new_without_default)]
    pub fn new(unregistered_message_handler: TUnregisteredMessageHandler) -> Self {
        Self {
            rpc_registry: CompletionRegistry::new(),
            is_alive: Arc::new(AtomicBool::new(true)),
            unregistered_message_handler,
        }
    }

    pub fn alive_handle(&self) -> Arc<AtomicBool> {
        self.is_alive.clone()
    }

    pub fn in_flight_submission_handle(&self) -> RpcRegistrar<Inbound> {
        self.rpc_registry.in_flight_submission_handle()
    }
}

impl<Inbound, TUnregisteredMessageHandler> Drop
    for RpcCompletionReactor<Inbound, TUnregisteredMessageHandler>
where
    Inbound: Message,
    TUnregisteredMessageHandler: UnregisteredMessageHandler<Inbound = Inbound>,
{
    fn drop(&mut self) {
        self.is_alive
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl<Inbound, TUnregisteredMessageHandler> MessageReactor
    for RpcCompletionReactor<Inbound, TUnregisteredMessageHandler>
where
    Inbound: Message,
    TUnregisteredMessageHandler: UnregisteredMessageHandler<Inbound = Inbound>,
{
    type Inbound = Inbound;

    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus {
        self.rpc_registry.take_new_rpc_lifecycle_actions();

        for message_from_the_network in messages.into_iter() {
            let message_id_from_the_network = message_from_the_network.message_id();
            match message_from_the_network.control_code() {
                ProtosocketControlCode::Normal => (),
                ProtosocketControlCode::Cancel => {
                    log::debug!("cancelling command {message_id_from_the_network}");
                    self.rpc_registry.deregister(message_id_from_the_network);
                    continue;
                }
                ProtosocketControlCode::End => {
                    log::debug!("end command {message_id_from_the_network}");
                    self.rpc_registry.deregister(message_id_from_the_network);
                    continue;
                }
            }
            match self.rpc_registry.entry(message_id_from_the_network) {
                Entry::Occupied(mut registered_rpc) => {
                    if let Completion::RemoteStreaming(stream) = registered_rpc.get_mut() {
                        if let Err(e) = stream.send(message_from_the_network) {
                            log::debug!("completion channel closed - did the client lose interest in this request? {e:?}");
                            registered_rpc.remove();
                        }
                    } else if let Completion::Unary(completion) = registered_rpc.remove() {
                        if let Err(e) = completion.send(Ok(message_from_the_network)) {
                            log::debug!("completion channel closed - did the client lose interest in this request? {e:?}");
                        }
                    } else {
                        panic!("unexpected command response type. Sorry, I wanted to borrow for streaming and remove by value for unary without doing 2 map lookups, so I couldn't match");
                    }
                }
                Entry::Vacant(_vacant_entry) => {
                    // Possibly a cancelled response if this is a client, and probably a new rpc if it's a server
                    log::debug!(
                        "command response for command that was not in flight: {message_id_from_the_network}"
                    );
                    self.unregistered_message_handler
                        .on_message(message_from_the_network, &mut self.rpc_registry);
                }
            }
        }
        ReactorStatus::Continue
    }
}

pub trait UnregisteredMessageHandler: Send + Unpin + 'static {
    type Inbound: Message;

    fn on_message(
        &mut self,
        message: Self::Inbound,
        rpc_registry: &mut CompletionRegistry<Self::Inbound>,
    );
}

#[derive(Debug)]
pub struct DoNothingMessageHandler<T: Message> {
    _phantom: PhantomData<T>,
}
impl<T: Message> UnregisteredMessageHandler for DoNothingMessageHandler<T> {
    type Inbound = T;

    fn on_message(
        &mut self,
        _message: Self::Inbound,
        _rpc_registry: &mut CompletionRegistry<Self::Inbound>,
    ) {
    }
}

impl<T: Message> Default for DoNothingMessageHandler<T> {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

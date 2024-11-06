use core::panic;
use std::{
    collections::{hash_map::Entry, HashMap},
    marker::PhantomData,
    sync::{atomic::AtomicBool, Arc},
};

use k_lock::Mutex;
use protosocket::{ConnectionBindings, MessageReactor, ReactorStatus};
use tokio::sync::{mpsc, oneshot};

use crate::{message::ProtosocketControlCode, Message};

#[derive(Debug)]
pub enum Completion<Response>
where
    Response: Message,
{
    Unary(oneshot::Sender<crate::Result<Response>>),
    Streaming(mpsc::UnboundedSender<Response>),
}

pub struct RpcCompletionConnectionBindings<Serializer, Deserializer>(
    PhantomData<(Serializer, Deserializer)>,
);
impl<Serializer, Deserializer> ConnectionBindings
    for RpcCompletionConnectionBindings<Serializer, Deserializer>
where
    Serializer: protosocket::Serializer + 'static,
    Deserializer: protosocket::Deserializer + 'static,
    Deserializer::Message: Message,
{
    type Deserializer = Deserializer;
    type Serializer = Serializer;
    type Reactor = RpcCompletionReactor<Deserializer::Message>;
}

#[derive(Debug)]
pub struct RpcCompletionReactor<Response>
where
    Response: Message,
{
    #[allow(clippy::type_complexity)]
    in_flight_submission: Arc<Mutex<HashMap<u64, Completion<Response>>>>,
    in_flight_buffer: HashMap<u64, Completion<Response>>,
    in_flight: HashMap<u64, Completion<Response>>,
    is_alive: Arc<AtomicBool>,
}
impl<Response> RpcCompletionReactor<Response>
where
    Response: Message,
{
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            in_flight_submission: Default::default(),
            in_flight_buffer: Default::default(),
            in_flight: Default::default(),
            is_alive: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn alive_handle(&self) -> Arc<AtomicBool> {
        self.is_alive.clone()
    }

    pub fn in_flight_submission_handle(&self) -> Arc<Mutex<HashMap<u64, Completion<Response>>>> {
        self.in_flight_submission.clone()
    }
}

impl<Response> Drop for RpcCompletionReactor<Response>
where
    Response: Message,
{
    fn drop(&mut self) {
        self.is_alive
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl<Response> MessageReactor for RpcCompletionReactor<Response>
where
    Response: Message,
{
    type Inbound = Response;

    // Yeah, I'm doing triple-buffering in here to keep a mutex as short as possible.
    // That involves a mutex, and I don't have a recourse if it's poisoned.
    #[allow(clippy::expect_used)]
    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> ReactorStatus {
        // only lock for the swap - this makes sure every time the in_flight_buffer is
        // used, it's for O(1) time.
        std::mem::swap(
            &mut self.in_flight_buffer,
            &mut *self
                .in_flight_submission
                .lock()
                .expect("brief internal mutex must work"),
        );
        self.in_flight.extend(self.in_flight_buffer.drain());

        for message in messages.into_iter() {
            let command_id = message.message_id();
            match message.control_code() {
                ProtosocketControlCode::Normal => (),
                ProtosocketControlCode::Cancel => {
                    log::debug!("cancelling command {command_id}");
                    self.in_flight.remove(&command_id);
                    continue;
                }
            }
            match self.in_flight.entry(command_id) {
                Entry::Occupied(mut registered_rpc) => {
                    if let Completion::Streaming(stream) = registered_rpc.get_mut() {
                        if let Err(e) = stream.send(message) {
                            log::debug!("completion channel closed - did the client lose interest in this request? {e:?}");
                            registered_rpc.remove();
                        }
                    } else if let Completion::Unary(completion) = registered_rpc.remove() {
                        if let Err(e) = completion.send(Ok(message)) {
                            log::debug!("completion channel closed - did the client lose interest in this request? {e:?}");
                        }
                    } else {
                        panic!("unexpected command response type. Sorry, I wanted to borrow for streaming and remove by value for unary without doing 2 map lookups, so I couldn't match");
                    }
                }
                Entry::Vacant(_vacant_entry) => {
                    // Possibly a cancelled command
                    log::debug!(
                        "command response for command that was not in flight: {command_id}"
                    );
                }
            }
        }
        ReactorStatus::Continue
    }
}

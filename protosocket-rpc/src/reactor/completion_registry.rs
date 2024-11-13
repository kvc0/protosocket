use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
};

use k_lock::Mutex;
use tokio::sync::{mpsc, oneshot};

use crate::Message;

#[derive(Debug, Default)]
pub struct CompletionRegistry<Inbound>
where
    Inbound: Message,
{
    #[allow(clippy::type_complexity)]
    in_flight_submission: Arc<Mutex<HashMap<u64, CompletionState<Inbound>>>>,
    in_flight_buffer: HashMap<u64, CompletionState<Inbound>>,
    in_flight: HashMap<u64, Completion<Inbound>>,
}

impl<Inbound> CompletionRegistry<Inbound>
where
    Inbound: Message,
{
    pub fn new() -> Self {
        Self {
            in_flight_submission: Default::default(),
            in_flight_buffer: Default::default(),
            in_flight: Default::default(),
        }
    }

    pub fn in_flight_submission_handle(&self) -> RpcRegistrar<Inbound> {
        RpcRegistrar {
            in_flight_submission: self.in_flight_submission.clone(),
        }
    }

    // Explicitly register a new completion
    #[allow(clippy::expect_used)]
    #[must_use]
    pub fn register_completion(
        &mut self,
        message_id: u64,
        completion: Completion<Inbound>,
    ) -> CompletionGuard<Inbound> {
        self.in_flight.insert(message_id, completion);
        CompletionGuard {
            in_flight_submission: self.in_flight_submission.clone(),
            message_id,
        }
    }

    pub fn take_new_rpc_lifecycle_actions(&mut self) {
        {
            let mut in_flight_submission = self
                .in_flight_submission
                .lock()
                .expect("brief internal mutex must work");
            if in_flight_submission.is_empty() {
                return;
            }
            // only lock for the swap - this makes sure every time the in_flight_buffer is
            // used, it's for O(1) time.
            std::mem::swap(&mut self.in_flight_buffer, &mut *in_flight_submission);
        }
        for (command_id, completion_state) in self.in_flight_buffer.drain() {
            match completion_state {
                CompletionState::InProgress(completion) => {
                    self.in_flight.insert(command_id, completion);
                }
                CompletionState::Done => {
                    log::debug!("command {command_id} done");
                    self.in_flight.remove(&command_id);
                }
            }
        }
    }

    pub fn deregister(&mut self, message_id: u64) {
        self.in_flight.remove(&message_id);
    }

    pub fn entry(&mut self, message_id: u64) -> Entry<'_, u64, Completion<Inbound>> {
        self.in_flight.entry(message_id)
    }
}

/// For removing a tracked rpc from the in-flight map when it is no longer needed
#[derive(Debug)]
pub struct CompletionGuard<Inbound>
where
    Inbound: Message,
{
    in_flight_submission: Arc<Mutex<HashMap<u64, CompletionState<Inbound>>>>,
    message_id: u64,
}
impl<Response> Drop for CompletionGuard<Response>
where
    Response: Message,
{
    fn drop(&mut self) {
        self.in_flight_submission
            .lock()
            .expect("brief internal mutex must work")
            // This doesn't result in a prompt wake of the reactor
            .insert(self.message_id, CompletionState::Done);
    }
}

#[derive(Debug, Clone)]
pub struct RpcRegistrar<Inbound>
where
    Inbound: Message,
{
    in_flight_submission: Arc<Mutex<HashMap<u64, CompletionState<Inbound>>>>,
}

impl<Inbound> RpcRegistrar<Inbound>
where
    Inbound: Message,
{
    // The triple-buffered message queue mutex is carefully controlled - it can't panic unless the memory allocator panics.
    // Probably the server should crash if that happens.
    // Note that this is just for tracking - you have to register the completion before sending the message, or else you might
    // miss the completion.
    #[allow(clippy::expect_used)]
    #[must_use]
    pub fn register_completion(
        &self,
        message_id: u64,
        completion: Completion<Inbound>,
    ) -> CompletionGuard<Inbound> {
        self.in_flight_submission
            .lock()
            .expect("brief internal mutex must work")
            .insert(message_id, CompletionState::InProgress(completion));
        CompletionGuard {
            in_flight_submission: self.in_flight_submission.clone(),
            message_id,
        }
    }
}

#[derive(Debug)]
pub enum Completion<Inbound>
where
    Inbound: Message,
{
    Unary(oneshot::Sender<crate::Result<Inbound>>),
    RemoteStreaming(mpsc::UnboundedSender<Inbound>),
}

#[derive(Debug)]
pub enum CompletionState<Inbound>
where
    Inbound: Message,
{
    InProgress(Completion<Inbound>),
    Done,
}

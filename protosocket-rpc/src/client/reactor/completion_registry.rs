use std::collections::{hash_map::Entry, HashMap};

use tokio::sync::oneshot;

use crate::{client::reactor::completion_reactor::RpcNotification, Message};

#[derive(Debug, Default)]
pub struct CompletionRegistry<Inbound>
where
    Inbound: Message,
{
    in_flight: HashMap<u64, Completion<Inbound>, ahash::RandomState>,
}

impl<Inbound> CompletionRegistry<Inbound>
where
    Inbound: Message,
{
    pub fn new() -> Self {
        Self {
            in_flight: Default::default(),
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
pub struct CompletionGuard<Inbound, Outbound>
where
    Inbound: Message,
    Outbound: Message,
{
    closed: bool,
    message_id: u64,
    raw_submission_queue: spillway::Sender<RpcNotification<Inbound, Outbound>>,
}

impl<Inbound, Outbound> CompletionGuard<Inbound, Outbound>
where
    Inbound: Message,
    Outbound: Message,
{
    pub(crate) fn new(
        message_id: u64,
        raw_submission_queue: spillway::Sender<RpcNotification<Inbound, Outbound>>,
    ) -> Self {
        Self {
            closed: false,
            message_id,
            raw_submission_queue,
        }
    }

    pub fn set_closed(&mut self) {
        self.closed = true;
    }
}

impl<Inbound, Outbound> Drop for CompletionGuard<Inbound, Outbound>
where
    Inbound: Message,
    Outbound: Message,
{
    fn drop(&mut self) {
        if !self.closed {
            if let Err(_e) = self
                .raw_submission_queue
                .send(RpcNotification::Cancel(self.message_id))
            {
                log::error!(
                    "unable to send cancellation for message - this will abandon server rpcs {}",
                    self.message_id
                );
            }
        }
    }
}

#[derive(Debug)]
pub enum Completion<Inbound> {
    Unary(oneshot::Sender<crate::Result<Inbound>>),
    RemoteStreaming(spillway::Sender<Inbound>),
}

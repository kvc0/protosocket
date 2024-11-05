use crate::Message;

#[derive(Debug)]
pub struct RpcDropGuard<Request>
where
    Request: Message,
{
    cancellation_submission_queue: tokio::sync::mpsc::Sender<Request>,
    message_id: u64,
    completed: bool,
}

impl<Request> RpcDropGuard<Request>
where
    Request: Message,
{
    pub fn new(
        cancellation_submission_queue: tokio::sync::mpsc::Sender<Request>,
        message_id: u64,
    ) -> Self {
        Self {
            cancellation_submission_queue,
            message_id,
            completed: false,
        }
    }

    /// Set this to avoid sending a cancellation message when the guard is dropped.
    pub fn set_complete(&mut self) {
        self.completed = true;
    }
}

impl<Request> Drop for RpcDropGuard<Request>
where
    Request: Message,
{
    fn drop(&mut self) {
        if !self.completed {
            if let Err(e) = self
                .cancellation_submission_queue
                .try_send(Request::cancelled(self.message_id))
            {
                log::warn!("failed to send cancellation message: {:?}", e);
            }
        }
    }
}

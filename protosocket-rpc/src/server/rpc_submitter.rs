use protosocket::{ConnectionBindings, Deserializer, MessageReactor};
use tokio::sync::mpsc;

use super::SocketService;

#[derive(Debug, Clone)]
pub struct RpcSubmitter<TSocketService>
where
    TSocketService: SocketService,
{
    sender: mpsc::UnboundedSender<<TSocketService::RequestDeserializer as Deserializer>::Message>,
}
impl<TSocketService> RpcSubmitter<TSocketService>
where
    TSocketService: SocketService,
{
    pub fn new() -> (
        Self,
        mpsc::UnboundedReceiver<<TSocketService::RequestDeserializer as Deserializer>::Message>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }
}

impl<TSocketService> MessageReactor for RpcSubmitter<TSocketService>
where
    TSocketService: SocketService,
{
    type Inbound = <TSocketService::RequestDeserializer as Deserializer>::Message;

    fn on_inbound_messages(
        &mut self,
        messages: impl IntoIterator<Item = Self::Inbound>,
    ) -> protosocket::ReactorStatus {
        for message in messages.into_iter() {
            match self.sender.send(message) {
                Ok(_) => (),
                Err(e) => {
                    log::warn!("failed to send message: {:?}", e);
                    return protosocket::ReactorStatus::Disconnect;
                }
            }
        }
        protosocket::ReactorStatus::Continue
    }
}

impl<TSocketService> ConnectionBindings for RpcSubmitter<TSocketService>
where
    TSocketService: SocketService,
{
    type Deserializer = TSocketService::RequestDeserializer;
    type Serializer = TSocketService::ResponseSerializer;
    type Reactor = RpcSubmitter<TSocketService>;
}

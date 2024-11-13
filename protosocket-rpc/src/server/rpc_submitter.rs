use protosocket::{ConnectionBindings, Deserializer, MessageReactor};
use tokio::sync::mpsc;

use super::SocketServer;

#[derive(Debug, Clone)]
pub struct RpcSubmitter<TSocketServer>
where
    TSocketServer: SocketServer,
{
    sender: mpsc::UnboundedSender<<TSocketServer::RequestDeserializer as Deserializer>::Message>,
}
impl<TSocketServer> RpcSubmitter<TSocketServer>
where
    TSocketServer: SocketServer,
{
    pub fn new() -> (
        Self,
        mpsc::UnboundedReceiver<<TSocketServer::RequestDeserializer as Deserializer>::Message>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }
}

impl<TSocketServer> MessageReactor for RpcSubmitter<TSocketServer>
where
    TSocketServer: SocketServer,
{
    type Inbound = <TSocketServer::RequestDeserializer as Deserializer>::Message;

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

impl<TSocketServer> ConnectionBindings for RpcSubmitter<TSocketServer>
where
    TSocketServer: SocketServer,
{
    type Deserializer = TSocketServer::RequestDeserializer;
    type Serializer = TSocketServer::ResponseSerializer;
    type Reactor = RpcSubmitter<TSocketServer>;
}

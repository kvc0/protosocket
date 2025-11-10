use protosocket::{Decoder, MessageReactor};
use tokio::sync::mpsc;

use super::SocketService;

/// A MessageReactor that sends RPCs along to a sink
#[derive(Debug, Clone)]
pub struct RpcSubmitter<TSocketService>
where
    TSocketService: SocketService,
{
    sender: mpsc::UnboundedSender<<TSocketService::RequestDecoder as Decoder>::Message>,
}
impl<TSocketService> RpcSubmitter<TSocketService>
where
    TSocketService: SocketService,
{
    pub fn new() -> (
        Self,
        mpsc::UnboundedReceiver<<TSocketService::RequestDecoder as Decoder>::Message>,
    ) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }
}

impl<TSocketService> MessageReactor for RpcSubmitter<TSocketService>
where
    TSocketService: SocketService,
{
    type Inbound = <TSocketService::RequestDecoder as Decoder>::Message;

    fn on_inbound_message(&mut self, message: Self::Inbound) -> protosocket::ReactorStatus {
        match self.sender.send(message) {
            Ok(_) => (),
            Err(e) => {
                log::warn!("failed to send message: {:?}", e);
                return protosocket::ReactorStatus::Disconnect;
            }
        }
        protosocket::ReactorStatus::Continue
    }
}

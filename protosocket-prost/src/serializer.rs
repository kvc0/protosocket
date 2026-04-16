use std::marker::PhantomData;

use protosocket::Serialize;

/// A stateless implementation of `Serialize` using `prost`
#[derive(Default, Debug)]
pub struct ProstSerializer<Message> {
    _phantom: PhantomData<Message>,
}

impl<Message> Serialize for ProstSerializer<Message>
where
    Message: prost::Message + std::fmt::Debug,
{
    type Message = Message;

    fn serialize_into_buffer(&mut self, message: Self::Message, buffer: &mut Vec<u8>) {
        match message.encode_length_delimited(buffer) {
            Ok(_) => {
                log::debug!("encoded {message:?}");
            }
            Err(e) => {
                log::error!("encoding error: {e:?}");
            }
        }
    }
}

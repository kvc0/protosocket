use std::marker::PhantomData;

use protosocket::{Decoder, DeserializeError};

/// A stateless implementation of `Decoder` using `prost`
#[derive(Debug, Default)]
pub struct ProstDecoder<Message> {
    _phantom: PhantomData<Message>,
}
impl<Message> Decoder for ProstDecoder<Message>
where
    Message: prost::Message + Default + std::fmt::Debug,
{
    type Message = Message;

    fn decode(
        &mut self,
        mut buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), DeserializeError> {
        match prost::decode_length_delimiter(buffer.chunk()) {
            Ok(message_length) => {
                if buffer.remaining() < message_length + prost::length_delimiter_len(message_length)
                {
                    return Err(DeserializeError::IncompleteBuffer {
                        next_message_size: message_length,
                    });
                }
            }
            Err(e) => {
                log::trace!("can't read a length delimiter {e:?}");
                return Err(DeserializeError::IncompleteBuffer {
                    next_message_size: 10,
                });
            }
        };

        let start = buffer.remaining();
        match <Self::Message as prost::Message>::decode_length_delimited(&mut buffer) {
            Ok(message) => {
                let length = start - buffer.remaining();
                log::debug!("decoded {length}: {message:?}");
                Ok((length, message))
            }
            Err(e) => {
                log::warn!("could not decode message: {e:?}");
                Err(DeserializeError::InvalidBuffer)
            }
        }
    }
}

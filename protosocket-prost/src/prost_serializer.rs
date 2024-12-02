use std::marker::PhantomData;

use protosocket::{DeserializeError, Deserializer, Serializer};

/// A stateless implementation of protosocket's `Serializer` and `Deserializer`
/// traits using `prost` for encoding and decoding protocol buffers messages.
#[derive(Default, Debug)]
pub struct ProstSerializer<Deserialized, Serialized> {
    pub(crate) _phantom: PhantomData<(Deserialized, Serialized)>,
}

impl<Deserialized, Serialized> Serializer for ProstSerializer<Deserialized, Serialized>
where
    Deserialized: prost::Message + Default + Unpin,
    Serialized: prost::Message + Unpin,
{
    type Message = Serialized;

    fn encode(&mut self, message: Self::Message, buffer: &mut Vec<u8>) {
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
impl<Deserialized, Serialized> Deserializer for ProstSerializer<Deserialized, Serialized>
where
    Deserialized: prost::Message + Default + Unpin,
    Serialized: prost::Message + Unpin,
{
    type Message = Deserialized;

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

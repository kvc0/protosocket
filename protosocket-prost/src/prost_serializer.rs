use std::marker::PhantomData;

use protosocket::{DeserializeError, Deserializer, Serializer};

/// A stateless implementation of protosocket's `Serializer` and `Deserializer`
/// traits using `prost` for encoding and decoding protocol buffers messages.
#[derive(Default, Debug)]
pub struct ProstSerializer<Request, Response> {
    pub(crate) _phantom: PhantomData<(Request, Response)>,
}

impl<Request, Response> Serializer for ProstSerializer<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
{
    type Message = Response;

    fn encode(&mut self, message: Self::Message, buffer: &mut impl bytes::BufMut) {
        match message.encode_length_delimited(buffer) {
            Ok(_) => {
                log::trace!("encoded reply {message:?}");
            }
            Err(e) => {
                log::error!("encoding error: {e:?}");
            }
        }
    }
}
impl<Request, Response> Deserializer for ProstSerializer<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
{
    type Message = Request;

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
                log::trace!("decoded request {length}: {message:?}");
                Ok((length, message))
            }
            Err(e) => {
                log::warn!("could not decode message: {e:?}");
                Err(DeserializeError::InvalidBuffer)
            }
        }
    }
}

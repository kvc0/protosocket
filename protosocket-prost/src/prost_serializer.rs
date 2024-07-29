use std::marker::PhantomData;

use protosocket_connection::{DeserializeError, Deserializer, Serializer};

#[derive(Default)]
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

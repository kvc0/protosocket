use std::marker::PhantomData;

use protosocket_connection::{DeserializeError, Deserializer, Serializer};

pub struct ProstSerializer<Request, Response> {
    pub(crate) _phantom: PhantomData<(Request, Response)>,
}

impl<Request, Response> Serializer for ProstSerializer<Request, Response>
where
    Request: prost::Message + Default + Unpin,
    Response: prost::Message + Unpin,
{
    type Response = Response;

    fn encode(&mut self, response: Self::Response, buffer: &mut impl bytes::BufMut) {
        match response.encode_length_delimited(buffer) {
            Ok(_) => {
                log::trace!("encoded reply");
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
    type Request = Request;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Request), DeserializeError> {
        match prost::decode_length_delimiter(buffer.chunk()) {
            Ok(length) => {
                log::trace!("reading {length} bytes from buffer");
                match <Self::Request as prost::Message>::decode_length_delimited(buffer) {
                    Ok(message) => {
                        log::trace!("decoded request");
                        Ok((0, message))
                    }
                    Err(e) => {
                        log::debug!("could not decode message: {e:?}");
                        Err(DeserializeError::InvalidBuffer)
                    }
                }
            }
            Err(e) => {
                log::debug!("could not decode message length: {e:?}");
                Err(DeserializeError::InvalidBuffer)
            }
        }
    }
}

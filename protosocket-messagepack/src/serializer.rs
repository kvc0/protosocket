use std::marker::PhantomData;

/// A serializer that takes a `serde::Serialize` T and implements
/// `protosocket::Serialize`. You can use this with a `protosocket`
/// Connection or rpc.
#[derive(Debug)]
pub struct MessagePackSerializer<T> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T> Default for MessagePackSerializer<T> {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<T> protosocket::Serialize for MessagePackSerializer<T>
where
    T: serde::Serialize + std::fmt::Debug,
{
    type Message = T;

    fn serialize_into_buffer(&mut self, message: Self::Message, buffer: &mut Vec<u8>) {
        log::debug!("encoding {message:?}");
        // reserve length prefix
        buffer.extend_from_slice(&[0; 5]);
        rmp_serde::encode::write(buffer, &message).expect("messages must be encodable");
        let len = buffer.len();
        unsafe {
            buffer.set_len(0);
        }
        rmp::encode::write_u32(buffer, len as u32 - 5).expect("message length is encodable");
        unsafe {
            buffer.set_len(len);
        }
    }
}

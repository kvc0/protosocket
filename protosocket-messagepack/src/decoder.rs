use std::marker::PhantomData;
use std::io::Read;

/// A deserializer that takes a serde deserializable T and implements
/// `protosocket::Decoder`. You can use this with a `protosocket`
/// Connection or rpc.
#[derive(Debug)]
pub struct ProtosocketMessagePackDecoder<T> {
    _phantom: std::marker::PhantomData<T>,
    state: State,
}

impl<T> Default for ProtosocketMessagePackDecoder<T> {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
            state: Default::default(),
        }
    }
}

#[derive(Debug, Default, Copy, Clone)]
enum State {
    #[default]
    Waiting,
    ReadingLength(u32),
}

impl<T> protosocket::Decoder for ProtosocketMessagePackDecoder<T>
where
    T: serde::de::DeserializeOwned + std::fmt::Debug,
{
    type Message = T;

    fn decode(
        &mut self,
        buffer: impl bytes::Buf,
    ) -> std::result::Result<(usize, Self::Message), protosocket::DeserializeError> {
        let start_remaining = buffer.remaining();
        let mut reader = buffer.reader();
        let length = match self.state {
            State::Waiting => {
                // 1 byte for the number tag, 4 bytes for the message length
                if start_remaining < 5 {
                    return Err(protosocket::DeserializeError::IncompleteBuffer {
                        next_message_size: 5,
                    });
                }
                let length: u32 = match rmp::decode::read_u32(&mut reader) {
                    Ok(length) => length,
                    Err(e) => {
                        log::error!("decode length error: {e:?}");
                        return Err(protosocket::DeserializeError::InvalidBuffer);
                    }
                };
                self.state = State::ReadingLength(length);
                length
            }
            State::ReadingLength(length) => {
                let _ = reader.read(&mut [0; 5]).expect("skip parsing");
                length
            }
        };
        if start_remaining < (length + 5) as usize {
            return Err(protosocket::DeserializeError::IncompleteBuffer {
                next_message_size: (length + 5) as usize,
            });
        }
        self.state = State::Waiting;

        rmp_serde::decode::from_read(&mut reader)
            .map_err(|e| {
                log::error!("decode error length {length}: {e:?}");
                protosocket::DeserializeError::InvalidBuffer
            })
            .map(|message| {
                let buffer = reader.into_inner();
                let length = start_remaining - buffer.remaining();
                log::debug!("decoded {length}: {message:?}");
                (length, message)
            })
    }
}

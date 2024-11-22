/// A protosocket message.
pub trait Message: std::fmt::Debug + Send + Unpin + 'static {
    /// This is used to relate requests to responses. An RPC response has the same id as the request that generated it.
    fn message_id(&self) -> u64;

    /// Set the protosocket behavior of this message.
    fn control_code(&self) -> ProtosocketControlCode;

    /// This is used to relate requests to responses. An RPC response has the same id as the request that generated it.
    /// When the message is sent, protosocket will set this value.
    fn set_message_id(&mut self, message_id: u64);

    /// Create a message with a message with a cancel control code - used by the framework to handle cancellation.
    fn cancelled(message_id: u64) -> Self;

    /// Create a message with a message with an ended control code - used by the framework to handle streaming completion.
    fn ended(message_id: u64) -> Self;
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum ProtosocketControlCode {
    /// No special behavior
    Normal = 0,
    /// Cancel processing the message with this message's id
    Cancel = 1,
    /// End processing the message with this message's id - for response streaming
    End = 2,
}

impl ProtosocketControlCode {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Normal,
            1 => Self::Cancel,
            2 => Self::End,
            _ => Self::Cancel,
        }
    }

    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Cancel => 1,
            Self::End => 2,
        }
    }
}

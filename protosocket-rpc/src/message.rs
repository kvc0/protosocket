/// A protosocket message.
pub trait Message: std::fmt::Debug + Send + 'static {
    /// This is used to relate requests to responses. An RPC response has the same id as the request that generated it.
    fn message_id(&self) -> u64;

    /// Set the protosocket behavior of this message.
    fn control_code(&self) -> ProtosocketControlCode;

    /// Create a message with a message with a cancel control code - used by the framework to handle cancellation.
    fn cancelled(message_id: u64) -> Self;
}

#[derive(Debug, Clone, Copy)]
pub enum ProtosocketControlCode {
    /// No special behavior
    Normal,
    /// Cancel processing the message with this message's id
    Cancel,
}

impl ProtosocketControlCode {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Normal,
            1 => Self::Cancel,
            _ => Self::Cancel,
        }
    }

    pub fn as_u8(&self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Cancel => 1,
        }
    }
}

/// Errors that can occur when deserializing a message.
#[derive(Debug, thiserror::Error)]
pub enum DeserializeError {
    /// Buffer will be retained and you will be called again later with more bytes
    #[error("Need more bytes to decode the next message")]
    IncompleteBuffer {
        /// This is a hint to the connection for how many more bytes should be read.
        /// You may be called again before you get another buffer with at least this
        /// many bytes.
        next_message_size: usize,
    },
    /// Buffer will be discarded
    #[error("Bad buffer")]
    InvalidBuffer,
    /// distance will be skipped
    #[error("Skip message")]
    SkipMessage {
        /// If a message is not to be serviced, you can skip it. This is how many
        /// bytes will be skipped.
        /// You may be called again before this message is skipped and you may need
        /// to repeat the skip.
        distance: usize,
    },
}

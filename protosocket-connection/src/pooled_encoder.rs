use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

use crate::Encoder;

/// Raw serializer for a buffer pool
pub trait Serialize {
    type Message;

    /// Write a message into a pooled buffer
    fn serialize_into_buffer(&mut self, message: Self::Message, buffer: &mut Vec<u8>);
}

/// An encoder that wraps a serializer, offering it raw byte vectors. These vectors
/// are reset and reused to minimize allocation cost.
pub struct PooledEncoder<TSerializer> {
    serializer: TSerializer,
    reused_buffers: Arc<crossbeam::queue::ArrayQueue<Vec<u8>>>,
}

impl<TSerializer> PooledEncoder<TSerializer>
where
    TSerializer: Serialize,
{
    /// Create a pooled encoder with default pool size.
    ///
    /// Note that small pool sizes are likely to do well even on heavily utilized systems.
    /// If the demand for concurrency is higher than the pool's size, then the pool will still
    /// settle to reuse with the additional concurrency held in flight. I.e., allocations are
    /// done in response to the first derivative of demand increase.
    pub fn new(serializer: TSerializer) -> Self {
        Self::new_with_pool_size(8, serializer)
    }

    /// Create a pooled encoder with explicit pool size.
    pub fn new_with_pool_size(pool_size: usize, serializer: TSerializer) -> Self {
        Self {
            serializer,
            reused_buffers: Arc::new(crossbeam::queue::ArrayQueue::new(pool_size)),
        }
    }
}

impl<TSerializer> Default for PooledEncoder<TSerializer>
where
    TSerializer: Serialize + Default,
{
    fn default() -> Self {
        Self::new_with_pool_size(8, TSerializer::default())
    }
}

impl<TSerializer> Encoder for PooledEncoder<TSerializer>
where
    TSerializer: Serialize,
{
    type Message = TSerializer::Message;
    type Serialized = Reusable;

    fn encode(&mut self, message: Self::Message) -> Self::Serialized {
        let mut buffer = self.reused_buffers.pop().unwrap_or_default();
        self.serializer.serialize_into_buffer(message, &mut buffer);
        Reusable::new(buffer, self.reused_buffers.clone())
    }
}

/// A reusable wrapper for a serializer buffer, which can be treated as a bytes::Buf.
#[derive(Debug, Clone)]
pub struct Reusable {
    inner: Vec<u8>,
    cursor: usize,
    reused_buffers: Arc<crossbeam::queue::ArrayQueue<Vec<u8>>>,
}

impl Reusable {
    fn new(buffer: Vec<u8>, reused_buffers: Arc<crossbeam::queue::ArrayQueue<Vec<u8>>>) -> Self {
        Self {
            inner: buffer,
            cursor: 0,
            reused_buffers,
        }
    }
}

impl Drop for Reusable {
    fn drop(&mut self) {
        self.inner.clear();
        // don't worry about it if the reuse queue is full
        let _ = self.reused_buffers.push(std::mem::take(&mut self.inner));
    }
}

impl bytes::Buf for Reusable {
    #[inline(always)]
    fn remaining(&self) -> usize {
        self.inner.len() - self.cursor
    }

    #[inline(always)]
    fn chunk(&self) -> &[u8] {
        &self.inner[self.cursor..]
    }

    #[inline(always)]
    fn advance(&mut self, cnt: usize) {
        assert!(self.inner.len() <= self.cursor + cnt);
        self.cursor += cnt;
    }

    #[inline(always)]
    fn chunks_vectored<'a>(&'a self, dst: &mut [std::io::IoSlice<'a>]) -> usize {
        if dst.is_empty() || self.cursor == self.inner.len() {
            0
        } else {
            dst[0] = std::io::IoSlice::new(self.chunk());
            1
        }
    }

    #[inline(always)]
    fn has_remaining(&self) -> bool {
        self.inner.len() != self.cursor
    }
}

impl Deref for Reusable {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Reusable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

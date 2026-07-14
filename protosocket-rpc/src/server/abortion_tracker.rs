use std::collections::HashMap;

use crate::server::rpc_stream::RpcAbortHandle;

#[derive(Default)]
pub struct AbortionTracker {
    aborts: HashMap<u64, RpcAbortHandle, ahash::RandomState>,
}

impl std::fmt::Debug for AbortionTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbortionTracker")
            .field("aborts", &self.aborts)
            .finish()
    }
}

impl AbortionTracker {
    pub fn register(&mut self, id: u64, handle: RpcAbortHandle) -> Option<RpcAbortHandle> {
        self.aborts.insert(id, handle)
    }

    pub fn take_abort(&mut self, id: u64) -> Option<RpcAbortHandle> {
        self.aborts.remove(&id)
    }
}

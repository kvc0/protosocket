use std::collections::HashMap;

use crate::server::abortable::IdentifiableAbortHandle;

#[derive(Default)]
pub struct AbortionTracker {
    aborts: HashMap<u64, IdentifiableAbortHandle, ahash::RandomState>,
}

impl std::fmt::Debug for AbortionTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbortionTracker")
            .field("aborts", &self.aborts)
            .finish()
    }
}

impl AbortionTracker {
    pub fn register(
        &mut self,
        id: u64,
        handle: IdentifiableAbortHandle,
    ) -> Option<IdentifiableAbortHandle> {
        self.aborts.insert(id, handle)
    }

    pub fn take_abort(&mut self, id: u64) -> Option<IdentifiableAbortHandle> {
        self.aborts.remove(&id)
    }
}

use std::collections::HashMap;

use k_lock::Mutex;

use crate::server::abortable::IdentifiableAbortHandle;

const SLOTS: usize = 4;

#[derive(Default)]
pub struct AbortionTracker {
    aborts: [Mutex<HashMap<u64, IdentifiableAbortHandle, ahash::RandomState>>; SLOTS],
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
        &self,
        id: u64,
        handle: IdentifiableAbortHandle,
    ) -> Option<IdentifiableAbortHandle> {
        self.aborts[id as usize & SLOTS]
            .lock()
            .expect("must not be poisoned")
            .insert(id, handle)
    }

    pub fn take_abort(&self, id: u64) -> Option<IdentifiableAbortHandle> {
        self.aborts[id as usize & SLOTS]
            .lock()
            .expect("must not be poisoned")
            .remove(&id)
    }
}

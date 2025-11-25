use std::sync::Arc;

use crate::shared::Shared;

pub struct Sender<T> {
    chute: usize,
    shared: Arc<Shared<T>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.add_sender();
        Self {
            chute: self.shared.choose_chute(),
            shared: self.shared.clone(),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let was = self.shared.drop_sender();
        if was == 1 {
            self.shared.wake();
        }
    }
}

impl<T> std::fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender")
            .field("chute", &self.chute)
            .finish()
    }
}

impl<T> Sender<T> {
    pub(crate) fn new(shared: Arc<Shared<T>>) -> Self {
        shared.add_sender();
        Self {
            chute: shared.choose_chute(),
            shared,
        }
    }

    pub fn send(&self, value: T) -> Result<(), T> {
        self.shared.send(self.chute, value)
    }
}

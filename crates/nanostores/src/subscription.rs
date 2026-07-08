use std::sync::Mutex;

/// RAII subscription guard. Dropping it unregisters the callback.
pub struct Subscription {
    cleanup: Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>,
}

impl Subscription {
    pub(crate) fn new(cleanup: impl FnOnce() + Send + 'static) -> Self {
        Self {
            cleanup: Mutex::new(Some(Box::new(cleanup))),
        }
    }

    /// Keep the subscription active forever.
    pub fn detach(self) {
        self.cleanup.lock().expect("subscription poisoned").take();
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.lock().expect("subscription poisoned").take() {
            cleanup();
        }
    }
}

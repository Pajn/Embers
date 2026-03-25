use std::sync::OnceLock;

use tokio::sync::{Mutex, MutexGuard};

pub struct IntegrationTestLock(Mutex<()>);

impl IntegrationTestLock {
    pub async fn lock(&self) -> MutexGuard<'_, ()> {
        self.0.lock().await
    }

    pub fn blocking_lock(&self) -> MutexGuard<'_, ()> {
        self.0.blocking_lock()
    }
}

pub fn integration_test_lock() -> &'static IntegrationTestLock {
    static LOCK: OnceLock<IntegrationTestLock> = OnceLock::new();
    LOCK.get_or_init(|| IntegrationTestLock(Mutex::new(())))
}

use std::sync::OnceLock;

use tokio::sync::{Mutex, MutexGuard};

pub struct IntegrationTestLock(Mutex<()>);

impl IntegrationTestLock {
    pub async fn lock(&self) -> MutexGuard<'_, ()> {
        self.0.lock().await
    }

    /// Uses `tokio::sync::Mutex::blocking_lock()` and will panic if called from an async context,
    /// including a Tokio runtime thread or an `#[tokio::test]`. Use this only from synchronous
    /// tests or non-async threads, and await `lock()` instead inside async tests.
    pub fn blocking_lock(&self) -> MutexGuard<'_, ()> {
        self.0.blocking_lock()
    }
}

pub fn integration_test_lock() -> &'static IntegrationTestLock {
    static LOCK: OnceLock<IntegrationTestLock> = OnceLock::new();
    LOCK.get_or_init(|| IntegrationTestLock(Mutex::new(())))
}

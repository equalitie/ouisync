use std::ops::{Deref, DerefMut};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::async_counter::{AsyncCounter, AsyncCounterFuture, AsyncCounterGuard};

/// Async read-write lock which allows readers to be notified about any interested writer so that
/// they can cooperatively yield the lock to them.
pub(crate) struct CoopRwLock<T> {
    inner: RwLock<T>,
    write_interests: AsyncCounter,
}

impl<T> CoopRwLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: RwLock::new(value),
            write_interests: AsyncCounter::new(),
        }
    }

    /// Acquires a read (shared) access to this lock. Returns a RAII guard that releases the lock on
    /// drop and also a future that can be awaited to get notified about any interested writers.
    pub async fn read(&self) -> (CoopRwLockReadGuard<'_, T>, AsyncCounterFuture<'_>) {
        let inner = self.inner.read().await;
        let inner = CoopRwLockReadGuard { inner };

        let interested = self.write_interests.wait_nonzero();

        (inner, interested)
    }

    /// Acquires a write (exclusive) access to this lock. If any read locks are currently being
    /// held, they get notified about this write attempt so they can decide to yield the lock to
    /// this task.
    pub async fn write(&self) -> CoopRwLockWriteGuard<'_, T> {
        let interest = self.write_interests.scoped_increment();
        let inner = self.inner.write().await;

        CoopRwLockWriteGuard { inner, interest }
    }
}

pub(crate) struct CoopRwLockReadGuard<'a, T> {
    inner: RwLockReadGuard<'a, T>,
}

impl<'a, T> Deref for CoopRwLockReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

pub(crate) struct CoopRwLockWriteGuard<'a, T> {
    inner: RwLockWriteGuard<'a, T>,
    #[expect(dead_code)]
    interest: AsyncCounterGuard<'a>,
}

impl<'a, T> Deref for CoopRwLockWriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<'a, T> DerefMut for CoopRwLockWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use futures_util::FutureExt;

    use super::*;

    #[test]
    fn read_then_write() {
        let lock = CoopRwLock::new(1);

        let (read_guard, wants_write) = lock.read().now_or_never().unwrap();

        let mut write_guard = pin!(lock.write());
        assert!(write_guard.as_mut().now_or_never().is_none());

        assert!(wants_write.now_or_never().is_some());
        drop(read_guard);

        assert!(write_guard.now_or_never().is_some());
    }

    #[test]
    fn write_then_read() {
        let lock = CoopRwLock::new(1);

        let write_guard = lock.write().now_or_never().unwrap();

        let mut read = pin!(lock.read());
        assert!(read.as_mut().now_or_never().is_none());

        drop(write_guard);
        assert!(read.as_mut().now_or_never().is_some());
    }
}

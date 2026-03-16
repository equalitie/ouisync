use std::{
    ops::{Deref, DerefMut},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, ready},
};

use pin_project_lite::pin_project;
use tokio::sync::{Notify, RwLock, RwLockReadGuard, RwLockWriteGuard, futures::Notified};

/// Async read-write lock which allows readers to be notified about any interested writer so that
/// they can cooperatively yield the lock to them.
pub(crate) struct CoopRwLock<T> {
    inner: RwLock<T>,
    interest: WriteInterest,
}

impl<T> CoopRwLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: RwLock::new(value),
            interest: WriteInterest::new(),
        }
    }

    /// Acquires a read (shared) access to this lock. Returns a RAII guard that releases the lock on
    /// drop and also a future that can be awaited to get notified about any interested writers.
    pub async fn read(&self) -> (CoopRwLockReadGuard<'_, T>, WriteInterested<'_>) {
        let interested = self.interest.wait_not_interested().await;

        let inner = self.inner.read().await;
        let inner = CoopRwLockReadGuard { inner };

        (inner, interested)
    }

    /// Acquires a write (exclusive) access to this lock. If any read locks are currently being
    /// held, they get notified about this write attempt so they can decide to release the lock and
    /// yield to this task.
    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn write(&self) -> CoopRwLockWriteGuard<'_, T> {
        let interest = self.interest.set_interested();
        let inner = self.inner.write().await;

        CoopRwLockWriteGuard { inner, interest }
    }

    /// Same as [Self::write] but blocks instead of awaits.
    pub fn blocking_write(&self) -> CoopRwLockWriteGuard<'_, T> {
        let interest = self.interest.set_interested();
        let inner = self.inner.blocking_write();

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
    interest: WriteInterestGuard<'a>,
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

// Tracks tasks that want write access to the lock, including the one already holding the lock
// (if any).
struct WriteInterest {
    count: AtomicUsize,
    notify: Notify,
}

impl WriteInterest {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            notify: Notify::new(),
        }
    }

    // Mark this task as being interested in write access to the lock. Returns a RAII guard that
    // marks the task as no longer interested on drop.
    fn set_interested(&self) -> WriteInterestGuard<'_> {
        self.count.fetch_add(1, Ordering::Acquire);
        self.notify.notify_waiters();

        WriteInterestGuard(self)
    }

    // Waits until no tasks are interested in write access to the lock. Returns a future that can be
    // awaited until some task becomes interested in write access again.
    async fn wait_not_interested(&self) -> WriteInterested<'_> {
        loop {
            let notified = self.notify.notified();

            if self.count.load(Ordering::Relaxed) == 0 {
                break WriteInterested {
                    count: &self.count,
                    notify: &self.notify,
                    notified,
                };
            }

            notified.await;
        }
    }
}

struct WriteInterestGuard<'a>(&'a WriteInterest);

impl Drop for WriteInterestGuard<'_> {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::Release);
        self.0.notify.notify_waiters();
    }
}

pin_project! {
    /// Future that resolves when any task becomes interested in write access to the lock.
    pub(crate) struct WriteInterested<'a> {
        count: &'a AtomicUsize,
        notify: &'a Notify,
        #[pin]
        notified: Notified<'a>,
    }
}

impl Future for WriteInterested<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            if this.count.load(Ordering::Relaxed) > 0 {
                break Poll::Ready(());
            } else {
                ready!(this.notified.as_mut().poll(cx));
                this.notified.set(this.notify.notified());
            }
        }
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

//! Utilities for deadlock detection

use crate::scoped_task::{self, ScopedJoinHandle};
use std::{
    future::Future,
    ops::{Deref, DerefMut},
    panic::Location,
    time::Duration,
};
use tokio::time;

const DEADLOCK_WARNING_TIMEOUT: Duration = Duration::from_secs(5);

// Wrapper for various lock guard types which prints a log message when the lock is being
// acquired, released and when it's taking too long to become acquired (potential deadlock).
pub struct DeadlockGuard<T> {
    inner: T,
    _location: LogOnDrop,
    _handle: ScopedJoinHandle<()>,
}

impl<T> DeadlockGuard<T> {
    pub(crate) async fn wrap<F>(inner: F, location: &'static Location<'static>) -> Self
    where
        F: Future<Output = T>,
    {
        let (inner, handle) = acquire(inner, location).await;

        Self {
            inner,
            _location: LogOnDrop(location),
            _handle: handle,
        }
    }

    pub(crate) async fn try_wrap<F, E>(
        inner: F,
        location: &'static Location<'static>,
    ) -> Result<Self, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        let (inner, handle) = acquire(inner, location).await;

        Ok(Self {
            inner: inner?,
            _location: LogOnDrop(location),
            _handle: handle,
        })
    }

    #[cfg(test)]
    pub(crate) fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> Deref for DeadlockGuard<T>
where
    T: Deref,
{
    type Target = T::Target;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<T> DerefMut for DeadlockGuard<T>
where
    T: DerefMut,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}

async fn acquire<F>(
    inner: F,
    location: &'static Location<'static>,
) -> (F::Output, ScopedJoinHandle<()>)
where
    F: Future,
{
    tokio::pin!(inner);

    let inner = match time::timeout(DEADLOCK_WARNING_TIMEOUT, &mut inner).await {
        Ok(inner) => inner,
        Err(_) => {
            // Warn when we are taking too long to acquire the lock.
            log::warn!("lock at {}: excessive wait (deadlock?)", location);
            inner.await
        }
    };

    log::trace!("lock at {}: acquired", location);

    // Warn when we are taking too long holding the lock.
    let handle = scoped_task::spawn(async move {
        time::sleep(DEADLOCK_WARNING_TIMEOUT).await;
        log::warn!("lock at {}: excessive hold (deadlock?)", location);
    });

    (inner, handle)
}

struct LogOnDrop(&'static Location<'static>);

impl Drop for LogOnDrop {
    fn drop(&mut self) {
        log::trace!("lock at {}: released", self.0);
    }
}

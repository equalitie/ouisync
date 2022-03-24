//! Utilities for deadlock detection

use slab::Slab;
use std::{
    fmt,
    future::Future,
    ops::{Deref, DerefMut},
    panic::Location,
    sync::{Arc, Mutex as BlockingMutex},
    time::Duration,
};
use tokio::time;

const WARNING_TIMEOUT: Duration = Duration::from_secs(5);

// Wrapper for various lock guard types which logs a warning when a potential deadlock is detected.
pub struct DeadlockGuard<T> {
    inner: T,
    _acquire: Acquire,
}

impl<T> DeadlockGuard<T> {
    #[track_caller]
    pub(crate) fn wrap<F>(inner: F, tracker: DeadlockTracker) -> impl Future<Output = Self>
    where
        F: Future<Output = T>,
    {
        let acquire = tracker.acquire();

        async move {
            let inner = detect_deadlock(inner, &tracker).await;

            Self {
                inner,
                _acquire: acquire,
            }
        }
    }

    #[track_caller]
    pub(crate) fn try_wrap<F, E>(
        inner: F,
        tracker: DeadlockTracker,
    ) -> impl Future<Output = Result<Self, E>>
    where
        F: Future<Output = Result<T, E>>,
    {
        let acquire = tracker.acquire();

        async move {
            let inner = detect_deadlock(inner, &tracker).await;

            Ok(Self {
                inner: inner?,
                _acquire: acquire,
            })
        }
    }

    pub(crate) fn into_inner(self) -> T {
        self.inner
    }

    pub(crate) fn as_ref(&self) -> &T {
        &self.inner
    }

    pub(crate) fn as_mut(&mut self) -> &mut T {
        &mut self.inner
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

/// Tracks all locations when a given lock is currently being acquired.
#[derive(Clone)]
pub(crate) struct DeadlockTracker {
    locations: Arc<BlockingMutex<Slab<&'static Location<'static>>>>,
}

impl DeadlockTracker {
    pub fn new() -> Self {
        Self {
            locations: Arc::new(BlockingMutex::new(Slab::new())),
        }
    }

    #[track_caller]
    fn acquire(&self) -> Acquire {
        let key = self.locations.lock().unwrap().insert(Location::caller());

        Acquire {
            locations: self.locations.clone(),
            key,
        }
    }
}

impl fmt::Display for DeadlockTracker {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let locations = self.locations.lock().unwrap();
        let mut comma = false;

        for (_, location) in &*locations {
            write!(f, "{}{}", if comma { ", " } else { "" }, location)?;
            comma = true;
        }

        Ok(())
    }
}

struct DeadlockMessage<'a>(&'a DeadlockTracker);

impl fmt::Display for DeadlockMessage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "potential deadlock at {}", self.0)
    }
}

struct Acquire {
    locations: Arc<BlockingMutex<Slab<&'static Location<'static>>>>,
    key: usize,
}

impl Drop for Acquire {
    fn drop(&mut self) {
        self.locations.lock().unwrap().remove(self.key);
    }
}

async fn detect_deadlock<F>(inner: F, tracker: &DeadlockTracker) -> F::Output
where
    F: Future,
{
    warn_slow(DeadlockMessage(tracker), inner).await
}

/// Run `fut` into completion but if it takes more than `WARNING_TIMEOUT`, log the given warning
/// message.
pub(crate) async fn warn_slow<F, M>(message: M, fut: F) -> F::Output
where
    F: Future,
    M: fmt::Display,
{
    tokio::pin!(fut);

    match time::timeout(WARNING_TIMEOUT, &mut fut).await {
        Ok(output) => output,
        Err(_) => {
            log::warn!("{}", message);
            fut.await
        }
    }
}

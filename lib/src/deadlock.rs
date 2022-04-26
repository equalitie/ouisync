//! Utilities for deadlock detection

use backtrace::Backtrace;
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

struct LockLocation {
    // NOTE: In release build, the backtrace contains some useful information, but not the actual
    // line where the `acquire` function was called for, thus we include the `Location` as well as
    // it's cheap and useful.
    file_and_line: &'static Location<'static>,
    backtrace: Backtrace,
}

/// Tracks all locations when a given lock is currently being acquired.
#[derive(Clone)]
pub(crate) struct DeadlockTracker {
    locations: Arc<BlockingMutex<Slab<LockLocation>>>,
}

impl DeadlockTracker {
    pub fn new() -> Self {
        Self {
            locations: Arc::new(BlockingMutex::new(Slab::new())),
        }
    }

    #[track_caller]
    fn acquire(&self) -> Acquire {
        let file_and_line = Location::caller();
        let backtrace = Backtrace::new_unresolved();

        let key = self.locations.lock().unwrap().insert(LockLocation {
            file_and_line,
            backtrace,
        });

        Acquire {
            locations: self.locations.clone(),
            key,
        }
    }
}

impl fmt::Display for DeadlockTracker {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut locations = self.locations.lock().unwrap();

        for (_, location) in &mut *locations {
            location.backtrace.resolve();
            write!(f, "\n{}\n{:?}", location.file_and_line, location.backtrace)?;
        }

        Ok(())
    }
}

struct DeadlockMessage<'a>(&'a DeadlockTracker);

impl fmt::Display for DeadlockMessage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "potential deadlock at:{}", self.0)
    }
}

struct Acquire {
    locations: Arc<BlockingMutex<Slab<LockLocation>>>,
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

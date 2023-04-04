use super::{ExpectShortLifetime, WARNING_TIMEOUT};
use std::{
    future::Future,
    ops::{Deref, DerefMut},
    panic::Location,
};

/// Replacement for `tokio::sync::Mutex` instrumented for deadlock detection.
pub struct Mutex<T> {
    inner: tokio::sync::Mutex<T>,
}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(value),
        }
    }

    // NOTE: using `track_caller` so that the `Location` constructed inside points to where
    // this function is called and not inside it. Also using `impl Future` return instead of
    // `async fn` because `track_caller` doesn't work correctly with `async`.
    #[track_caller]
    pub fn lock(&self) -> impl Future<Output = MutexGuard<'_, T>> {
        let location = Location::caller();

        async move {
            let inner = self.inner.lock().await;
            let tracker = ExpectShortLifetime::new(WARNING_TIMEOUT, location);

            MutexGuard {
                inner,
                _tracker: tracker,
            }
        }
    }
}

pub struct MutexGuard<'a, T> {
    inner: tokio::sync::MutexGuard<'a, T>,
    _tracker: ExpectShortLifetime,
}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}

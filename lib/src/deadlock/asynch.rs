use crate::deadlock::{DeadlockGuard, DeadlockTracker};
use scoped_task::{spawn, ScopedJoinHandle};
use std::future::Future;
use std::{backtrace::Backtrace, fmt, panic::Location, time::Duration};
use tokio::time::sleep;

//------------------------------------------------------------------------------

/// Replacement for `tokio::sync::Mutex` instrumented for deadlock detection.
pub struct Mutex<T> {
    inner: tokio::sync::Mutex<T>,
    deadlock_tracker: DeadlockTracker,
}

impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(value),
            deadlock_tracker: DeadlockTracker::new(),
        }
    }

    // NOTE: using `track_caller` so that the `Location` constructed inside points to where
    // this function is called and not inside it. Also using `impl Future` return instead of
    // `async fn` because `track_caller` doesn't work correctly with `async`.
    #[track_caller]
    pub fn lock(&self) -> impl Future<Output = MutexGuard<'_, T>> {
        DeadlockGuard::wrap(self.inner.lock(), self.deadlock_tracker.clone())
    }
}

pub type MutexGuard<'a, T> = DeadlockGuard<tokio::sync::MutexGuard<'a, T>>;

//------------------------------------------------------------------------------

pub(crate) struct ExpectShortLifetime {
    _watcher: ScopedJoinHandle<()>,
}

impl ExpectShortLifetime {
    #[track_caller]
    pub fn new(max_lifetime: Duration) -> Self {
        let file_and_line = Location::caller();
        let backtrace = Backtrace::capture();
        Self {
            _watcher: spawn(async move {
                sleep(max_lifetime.into()).await;
                println!("Expected short lifetime, but exceeded {:?}", max_lifetime);
                println!("Location: {:?}", file_and_line);
                println!("Backtrace: {:?}", backtrace);
            }),
        }
    }
}

impl fmt::Debug for ExpectShortLifetime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ExpectShortLifetime")
    }
}

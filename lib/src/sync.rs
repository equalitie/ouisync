//! Synchronization utilities

use crate::deadlock::{DeadlockGuard, DeadlockTracker};
use std::future::Future;

/// MPMC broadcast channel
pub(crate) mod broadcast {
    use std::time::{Duration, Instant};
    use tokio::{select, sync::broadcast, time};

    /// Adapter for `Receiver` which limits the rate at which messages are received. The messages
    /// are not buffered - if the rate limit is exceeded, all but the last message are discarded.
    pub(crate) struct ThrottleReceiver<T> {
        rx: broadcast::Receiver<T>,
        interval: Duration,
        last_recv: Instant,
    }

    impl<T> ThrottleReceiver<T>
    where
        T: Clone,
    {
        pub fn new(inner: broadcast::Receiver<T>, interval: Duration) -> Self {
            Self {
                rx: inner,
                interval,
                last_recv: Instant::now() - interval,
            }
        }

        pub async fn recv(&mut self) -> Result<T, broadcast::error::RecvError> {
            let mut item = None;
            let end = self.last_recv + self.interval;

            if Instant::now() < end {
                loop {
                    select! {
                        _ = time::sleep_until(end.into()) => break,
                        result = self.rx.recv() => {
                            item = Some(result?);
                        }
                    }
                }
            }

            let item = if let Some(item) = item {
                item
            } else {
                self.rx.recv().await?
            };

            self.last_recv = Instant::now();

            Ok(item)
        }
    }
}

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

/// Similar to tokio::sync::watch, but has no initial value. Because there is no initial value the
/// API must be sligthly different. In particular, we don't have the `borrow` function.
pub(crate) mod uninitialized_watch {
    use tokio::sync::watch as w;

    pub struct Sender<T>(w::Sender<Option<T>>);

    impl<T> Sender<T> {
        pub fn send(&self, value: T) -> Result<(), w::error::SendError<T>> {
            // Unwrap OK because we know we just wrapped the value.
            self.0
                .send(Some(value))
                .map_err(|e| w::error::SendError(e.0.unwrap()))
        }

        pub fn subscribe(&self) -> Receiver<T> {
            Receiver(self.0.subscribe())
        }
    }

    #[derive(Clone)]
    pub struct Receiver<T>(w::Receiver<Option<T>>);

    impl<T: Clone> Receiver<T> {
        pub async fn changed(&mut self) -> Result<T, w::error::RecvError> {
            loop {
                if let Err(e) = self.0.changed().await {
                    return Err(e);
                }

                // Note: the w::Ref struct returned by `borrow` does not implement `Map`, so (I
                // think) we need to clone the value wrapped in `Option`.
                match &*self.0.borrow() {
                    // It's the initial value, we ignore that one.
                    None => continue,
                    Some(v) => return Ok(v.clone()),
                }
            }
        }
    }

    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = w::channel(None);
        (Sender(tx), Receiver(rx))
    }
}

/// Simple mpmc channel that signals when a sender drops. The sender is `Clone` so the channel can
/// be used to watch multiple values.
pub(crate) mod drop_notify {
    use super::uninitialized_watch;
    use std::sync::Arc;
    pub use tokio::sync::watch::error::RecvError;

    // TODO: consider creating a PR against tokio to make `watch::Sender` Clone.
    #[derive(Clone)]
    pub struct Sender(Arc<uninitialized_watch::Sender<()>>);

    impl Sender {
        pub fn new() -> Self {
            let (tx, _) = uninitialized_watch::channel();
            Self(Arc::new(tx))
        }
        pub fn subscribe(&self) -> Receiver {
            Receiver(self.0.subscribe())
        }
    }

    impl Default for Sender {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for Sender {
        fn drop(&mut self) {
            self.0.send(()).unwrap_or(())
        }
    }

    #[derive(Clone)]
    pub struct Receiver(uninitialized_watch::Receiver<()>);

    impl Receiver {
        /// Wait until one sender has been dropped. Returns an error if there are no more sender at
        /// the time this is called.
        pub async fn dropped(&mut self) -> Result<(), RecvError> {
            self.0.changed().await
        }
    }
}

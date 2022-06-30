//! Synchronization utilities

use crate::deadlock::{DeadlockGuard, DeadlockTracker};
use std::future::Future;

/// MPMC broadcast channel
pub(crate) mod broadcast {
    pub use async_broadcast::Receiver;
    use std::time::{Duration, Instant};
    use tokio::{select, sync::watch, task, time};

    /// Sender for a mpmc broadcast channel.
    ///
    /// This is a wrapper for `async_broadcast::Sender` which provides these additional features:
    ///
    /// 1. Receiver can be created directly from `Sender` (using `subscribe`), without needing to
    ///    keep a `Receiver` or `InactiveReceiver` around.
    /// 2. Calling `broadcast` when there are no subscribed receivers does not wait (block).
    #[derive(Clone)]
    pub struct Sender<T> {
        tx: async_broadcast::Sender<T>,
        rx: async_broadcast::InactiveReceiver<T>,
    }

    impl<T> Sender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        /// Create a broadcast channel with the specified capacity.
        pub fn new(capacity: usize) -> Self {
            let (tx, rx) = async_broadcast::broadcast(capacity);

            // HACK: drain the channel so that sending doesn't block when there are no
            // subscriptions.
            task::spawn({
                let mut rx = rx.clone();
                async move { while rx.recv().await.is_ok() {} }
            });

            Self {
                tx,
                rx: rx.deactivate(),
            }
        }

        /// Broadcast a message on the channel. The message will be received by all currently
        /// subscribed receivers. If the channel is full, then this function waits until it becomes
        /// non-full again. However, if there are currently no receivers, then this function
        /// returns immediately with `Ok`.
        pub async fn broadcast(&self, value: T) -> Result<(), async_broadcast::SendError<T>> {
            self.tx.broadcast(value).await?;
            Ok(())
        }

        /// Create a new receiver subscribed to this channel.
        pub fn subscribe(&self) -> async_broadcast::Receiver<T> {
            self.rx.activate_cloned()
        }

        /// Close the channel explicitly. This makes all subsequent receives to fail immediately.
        pub fn close(&self) {
            self.tx.close();
        }
    }

    /// Adapter for `Sender` which enables overflow of messages sent previously on this same sender.
    pub struct OverflowSender<T> {
        tx: watch::Sender<Option<T>>,
    }

    impl<T> OverflowSender<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        pub fn new(inner: Sender<T>) -> Self {
            let (tx, mut rx) = watch::channel::<Option<T>>(None);

            task::spawn(async move {
                while rx.changed().await.is_ok() {
                    let value = if let Some(value) = &*rx.borrow() {
                        value.clone()
                    } else {
                        continue;
                    };

                    if inner.broadcast(value).await.is_err() {
                        break;
                    }
                }
            });

            Self { tx }
        }

        pub fn broadcast(&self, value: T) -> Result<(), async_broadcast::SendError<T>> {
            self.tx
                .send(Some(value))
                .map_err(|watch::error::SendError(value)| {
                    async_broadcast::SendError(value.unwrap())
                })
        }
    }

    /// Adapter for `Receiver` which limits the rate at which messages are received. The messages
    /// are not buffered - if the rate limit is exceeded, all but the last message are discarded.
    pub(crate) struct ThrottleReceiver<T> {
        rx: Receiver<T>,
        interval: Duration,
        last_recv: Instant,
    }

    impl<T> ThrottleReceiver<T>
    where
        T: Clone,
    {
        pub fn new(inner: Receiver<T>, interval: Duration) -> Self {
            Self {
                rx: inner,
                interval,
                last_recv: Instant::now() - interval,
            }
        }

        pub async fn recv(&mut self) -> Result<T, async_broadcast::RecvError> {
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

/// Replacement for `tokio::sync::RwLock` instrumented for deadlock detection.
pub struct RwLock<T> {
    inner: tokio::sync::RwLock<T>,
    deadlock_tracker: DeadlockTracker,
}

impl<T> RwLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: tokio::sync::RwLock::new(value),
            deadlock_tracker: DeadlockTracker::new(),
        }
    }

    #[track_caller]
    pub fn read(&self) -> impl Future<Output = RwLockReadGuard<'_, T>> {
        DeadlockGuard::wrap(self.inner.read(), self.deadlock_tracker.clone())
    }

    #[track_caller]
    pub fn write(&self) -> impl Future<Output = RwLockWriteGuard<'_, T>> {
        DeadlockGuard::wrap(self.inner.write(), self.deadlock_tracker.clone())
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }
}

pub type RwLockReadGuard<'a, T> = DeadlockGuard<tokio::sync::RwLockReadGuard<'a, T>>;
pub type RwLockWriteGuard<'a, T> = DeadlockGuard<tokio::sync::RwLockWriteGuard<'a, T>>;

/// Similar to tokio::sync::watch, but has no initial value. Because there is no initial value the
/// API must be sligthly different. In particular, we don't have the `borrow` function.
pub mod uninitialized_watch {
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

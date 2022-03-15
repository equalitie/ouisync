//! Synchronization utilities

pub(crate) use self::instrumented::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// MPMC broadcast channel
pub(crate) mod broadcast {
    pub use async_broadcast::Receiver;
    use tokio::task;

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
        T: Clone + Send + 'static,
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
}

/// Synchronization objects instrumented for deadlock detection.
pub(super) mod instrumented {
    pub use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

    use crate::scoped_task::{self, ScopedJoinHandle};
    use std::{
        future::Future,
        ops::{Deref, DerefMut},
        panic::Location,
        time::Duration,
    };
    use tokio::{sync as upstream, time};

    const DEADLOCK_WARNING_TIMEOUT: Duration = Duration::from_secs(5);

    /// Instrumented replacement for `tokio::sync::Mutex`.
    pub struct Mutex<T>(upstream::Mutex<T>);

    impl<T> Mutex<T> {
        pub fn new(value: T) -> Self {
            Self(upstream::Mutex::new(value))
        }

        // NOTE: using `track_caller` so that the `Location` constructed inside points to where
        // this function is called and not inside it. Also using `impl Future` return instead of
        // `async fn` because `track_caller` doesn't work correctly with `async`.
        #[track_caller]
        pub fn lock(&self) -> impl Future<Output = MutexGuard<'_, T>> {
            Guard::new(self.0.lock(), Location::caller())
        }
    }

    pub type MutexGuard<'a, T> = Guard<upstream::MutexGuard<'a, T>>;

    // Wrapper for various lock guard types which prints a log message when the lock is being
    // acquired, released and when it's taking too long to become acquired (potential deadlock).
    pub struct Guard<T> {
        inner: T,
        location: &'static Location<'static>,
        _handle: ScopedJoinHandle<()>,
    }

    impl<T> Guard<T> {
        async fn new<F>(inner: F, location: &'static Location<'static>) -> Self
        where
            F: Future<Output = T>,
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

            Self {
                inner,
                location,
                _handle: handle,
            }
        }
    }

    impl<T> Drop for Guard<T> {
        fn drop(&mut self) {
            log::trace!("lock at {}: released", self.location);
        }
    }

    impl<T> Deref for Guard<T>
    where
        T: Deref,
    {
        type Target = T::Target;

        fn deref(&self) -> &Self::Target {
            self.inner.deref()
        }
    }

    impl<T> DerefMut for Guard<T>
    where
        T: DerefMut,
    {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.inner.deref_mut()
        }
    }
}

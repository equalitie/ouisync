//! Synchronization utilities

/// MPMC broadcast channel
pub mod broadcast {
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
                last_recv: Instant::now()
                    .checked_sub(interval)
                    .expect("Interval should be smaller than the time since epoch"),
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

/// Similar to tokio::sync::watch, but has no initial value. Because there is no initial value the
/// API must be sligthly different. In particular, we don't have the `borrow` function.
pub(crate) mod uninitialized_watch {
    use futures_util::{stream, Stream};
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
                self.0.changed().await?;

                // Note: the w::Ref struct returned by `borrow` does not implement `Map`, so (I
                // think) we need to clone the value wrapped in `Option`.
                match &*self.0.borrow() {
                    // It's the initial value, we ignore that one.
                    None => continue,
                    Some(v) => return Ok(v.clone()),
                }
            }
        }

        /// Returns a `Stream` that calls `changed` repeatedly and yields the returned values.
        pub fn as_stream(&mut self) -> impl Stream<Item = T> + '_ {
            stream::unfold(self, |rx| async {
                rx.changed().await.ok().map(|value| (value, rx))
            })
        }
    }

    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = w::channel(None);
        (Sender(tx), Receiver(rx))
    }
}

/// Use this class to `await` until something is `Drop`ped.
//
// Example:
//
//     let drop_awaitable = DropAwaitable::new();
//     let on_dropped = drop_awaitable.subscribe();
//
//     tokio::spawn(async move {
//       let _drop_awaitable = drop_awaitable; // Move it to this task.
//       // Do some async tasks.
//     });
//
//     on_dropped.recv().await;
//
pub struct DropAwaitable {
    tx: tokio::sync::watch::Sender<()>,
}

impl DropAwaitable {
    pub fn new() -> Self {
        Self {
            tx: tokio::sync::watch::channel(()).0,
        }
    }

    pub fn subscribe(&self) -> AwaitDrop {
        AwaitDrop {
            rx: self.tx.subscribe(),
        }
    }
}

impl Default for DropAwaitable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct AwaitDrop {
    rx: tokio::sync::watch::Receiver<()>,
}

impl AwaitDrop {
    pub async fn recv(mut self) {
        while self.rx.changed().await.is_ok() {}
    }
}

/// Wrapper that allows non-blocking concurrent read or replace of the underlying value.
pub(crate) mod atomic_slot {
    use std::{
        mem,
        ops::Deref,
        sync::{Arc, Mutex},
    };

    pub struct AtomicSlot<T>(Mutex<Arc<T>>);

    impl<T> AtomicSlot<T> {
        pub fn new(value: T) -> Self {
            Self(Mutex::new(Arc::new(value)))
        }

        /// Obtain read access to the underlying value without blocking.
        pub fn read(&self) -> Guard<T> {
            Guard(self.0.lock().unwrap().clone())
        }

        /// Atomically replace the current value with the provided one and returns the previous one.
        pub fn swap(&self, value: T) -> Guard<T> {
            let value = Arc::new(value);
            Guard(mem::replace(&mut *self.0.lock().unwrap(), value))
        }
    }

    /// Handle to provide read access to a value stored in `AtomicSlot`.
    pub struct Guard<T>(Arc<T>);

    impl<T> From<T> for Guard<T> {
        fn from(value: T) -> Self {
            Self(Arc::new(value))
        }
    }

    impl<T> Deref for Guard<T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            self.0.deref()
        }
    }
}

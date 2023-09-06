//! Synchronization utilities

use futures_util::StreamExt;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

/// Similar to tokio::sync::watch, but has no initial value. Because there is no initial value the
/// API must be sligthly different. In particular, we don't have the `borrow` function.
pub(crate) mod uninitialized_watch {
    use futures_util::{stream, Stream};
    use tokio::sync::watch as w;
    pub use w::error::RecvError;

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
    tx: watch::Sender<()>,
}

impl DropAwaitable {
    pub fn new() -> Self {
        Self {
            tx: watch::channel(()).0,
        }
    }

    pub fn subscribe(&self) -> AwaitDrop {
        AwaitDrop {
            rx: WatchStream::new(self.tx.subscribe()),
        }
    }
}

impl Default for DropAwaitable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AwaitDrop {
    rx: WatchStream<()>,
}

impl Future for AwaitDrop {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        while ready!(self.rx.poll_next_unpin(cx)).is_some() {}
        Poll::Ready(())
    }
}

/// Wrapper that allows non-blocking concurrent read or replace of the underlying value.
pub(crate) mod atomic_slot {
    use crate::deadlock::BlockingMutex;
    use std::{mem, ops::Deref, sync::Arc};

    pub struct AtomicSlot<T>(BlockingMutex<Arc<T>>);

    impl<T> AtomicSlot<T> {
        pub fn new(value: T) -> Self {
            Self(BlockingMutex::new(Arc::new(value)))
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

/// Similar to `tokio::sync::broadcast` but does not enqueue the values. Instead, it "accumulates"
/// them into a set. Advantages include that one doesn't need to specify the recv buffer size and
/// the `insert` (analogue to `broadcast::send`) is non-async and never fails.
pub(crate) mod broadcast_hash_set {
    use super::uninitialized_watch;
    pub use super::uninitialized_watch::RecvError;
    use std::{
        collections::{HashMap, HashSet},
        hash::Hash,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    #[derive(Clone)]
    pub struct Sender<T> {
        shared: Arc<Shared<T>>,
    }

    impl<T: Eq + Hash + Clone> Sender<T> {
        pub fn insert(&self, value: &T) {
            let mut receivers = self.shared.receivers.lock().unwrap();

            for (_id, receiver) in receivers.iter_mut() {
                if receiver.1.insert(value.clone()) {
                    receiver.0.send(()).unwrap_or(());
                }
            }
        }

        pub fn subscribe(&self) -> Receiver<T> {
            let id = self.shared.id_generator.fetch_add(1, Ordering::Relaxed);
            let (watch_tx, watch_rx) = uninitialized_watch::channel();

            let mut receivers = self.shared.receivers.lock().unwrap();

            receivers.insert(id, (watch_tx, HashSet::new()));

            Receiver {
                id,
                shared: self.shared.clone(),
                watch_rx,
            }
        }
    }

    pub struct Receiver<T> {
        id: usize,
        shared: Arc<Shared<T>>,
        watch_rx: uninitialized_watch::Receiver<()>,
    }

    impl<T> Receiver<T> {
        pub async fn changed(&mut self) -> Result<HashSet<T>, RecvError> {
            self.watch_rx.changed().await?;

            let mut receivers = self.shared.receivers.lock().unwrap();

            // Unwrap is OK because the entry must exists for as long as this `Receiver` exists.
            let old_set = &mut receivers.get_mut(&self.id).unwrap().1;
            let mut new_set = HashSet::new();

            if !old_set.is_empty() {
                std::mem::swap(&mut new_set, old_set);
            }

            Ok(new_set)
        }
    }

    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            self.shared.receivers.lock().unwrap().remove(&self.id);
        }
    }

    struct Shared<T> {
        id_generator: AtomicUsize,
        receivers: Mutex<HashMap<usize, (uninitialized_watch::Sender<()>, HashSet<T>)>>,
    }

    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        let id_generator = AtomicUsize::new(0);
        let mut receivers = HashMap::new();

        let id = id_generator.fetch_add(1, Ordering::Relaxed);
        let (watch_tx, watch_rx) = uninitialized_watch::channel();

        receivers.insert(id, (watch_tx, HashSet::new()));

        let shared = Arc::new(Shared {
            id_generator,
            receivers: Mutex::new(receivers),
        });

        (
            Sender {
                shared: shared.clone(),
            },
            Receiver {
                id,
                shared,
                watch_rx,
            },
        )
    }
}

pub(crate) mod stream {
    use futures_util::{stream::Fuse, Stream, StreamExt};
    use indexmap::IndexSet;
    use pin_project_lite::pin_project;
    use std::{
        future::Future,
        hash::Hash,
        pin::Pin,
        task::{ready, Context, Poll},
    };
    use tokio::time::{self, Duration, Sleep};

    pin_project! {
        /// Rate-limitting stream adapter.
        ///
        /// ```ignore
        /// in:       |a|a|a|a|a| | | | | |a|a|a|a| | | |
        /// debounce: | | | | | | |a| | | | | | | | |a| |
        /// throttle: |a| |a| |a| | | | | |a| |a| |a| | |
        /// ```
        ///
        /// Multiple occurences of the same item within the rate-limit period are reduced to a
        /// single one but distinct items are preserved:
        ///
        /// ```ignore
        /// in:        |a|b|a|b| | | | |
        /// debounced: | | | | | |a|b| |
        /// throttle:  |a| |a|b| | | | |
        /// ```
        pub struct RateLimit<S>
        where
            S: Stream,
        {
            #[pin]
            inner: Fuse<S>,
            strategy: RateLimitStrategy,
            period: Duration,
            items: IndexSet<S::Item>,
            #[pin]
            sleep: Option<Sleep>,
        }
    }

    impl<S> RateLimit<S>
    where
        S: Stream,
    {
        pub fn new(inner: S, strategy: RateLimitStrategy, period: Duration) -> Self {
            Self {
                inner: inner.fuse(),
                strategy,
                period,
                items: IndexSet::new(),
                sleep: None,
            }
        }

        pub fn debounce(inner: S, period: Duration) -> Self {
            Self::new(inner, RateLimitStrategy::Debounce, period)
        }

        pub fn throttle(inner: S, period: Duration) -> Self {
            Self::new(inner, RateLimitStrategy::Throttle, period)
        }
    }

    impl<S> Stream for RateLimit<S>
    where
        S: Stream,
        S::Item: Hash + Eq,
    {
        type Item = S::Item;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut this = self.project();

            loop {
                if this.sleep.is_none() {
                    if let Some(item) = this.items.pop() {
                        return Poll::Ready(Some(item));
                    }
                }

                match this.inner.as_mut().poll_next(cx) {
                    Poll::Ready(Some(item)) => match this.strategy {
                        RateLimitStrategy::Debounce => {
                            this.sleep.set(Some(time::sleep(*this.period)));
                            this.items.insert(item);
                        }
                        RateLimitStrategy::Throttle => {
                            if this.sleep.is_none() {
                                this.sleep.set(Some(time::sleep(*this.period)));
                                return Poll::Ready(Some(item));
                            } else {
                                this.items.insert(item);
                            }
                        }
                    },
                    Poll::Ready(None) => {
                        if this.sleep.is_some() {
                            this.sleep.set(None);
                            this.items.reverse(); // keep the original order
                            continue;
                        } else {
                            return Poll::Ready(None);
                        }
                    }
                    Poll::Pending => (),
                }

                if let Some(sleep) = this.sleep.as_mut().as_pin_mut() {
                    ready!(sleep.poll(cx));
                    this.sleep.set(None);
                    this.items.reverse(); // keep the original order
                } else {
                    return Poll::Pending;
                }
            }
        }
    }

    #[derive(Copy, Clone, Debug)]
    pub enum RateLimitStrategy {
        Debounce,
        Throttle,
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use futures_util::{future, stream, StreamExt};
        use std::fmt::Debug;
        use tokio::{sync::mpsc, time::Instant};

        #[tokio::test(start_paused = true)]
        async fn rate_limit_equal_items() {
            let input = [
                (ms(0), 0),
                (ms(100), 0),
                (ms(100), 0),
                (ms(1500), 0),
                (ms(0), 0),
            ];

            // unlimitted
            let (tx, rx) = mpsc::channel(1);
            let expected = [
                (0, ms(0)),
                (0, ms(100)),
                (0, ms(200)),
                (0, ms(1700)),
                (0, ms(1700)),
            ];
            future::join(produce(tx, input), verify(into_stream(rx), expected)).await;

            // debounced
            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(1200)), (0, ms(2700))];
            future::join(
                produce(tx, input),
                verify(RateLimit::debounce(into_stream(rx), ms(1000)), expected),
            )
            .await;

            // FIXME:
            // // throttled
            // let (tx, rx) = mpsc::channel(1);
            // let expected = [(0, ms(0)), (0, ms(1000)), (0, ms(1700)), (0, ms(2700))];
            // future::join(
            //     produce(tx, input),
            //     verify(RateLimit::throttle(into_stream(rx), ms(1000)), expected),
            // )
            // .await;
        }

        #[tokio::test(start_paused = true)]
        async fn rate_limit_inequal_items() {
            let input = [(ms(0), 0), (ms(0), 1), (ms(0), 0), (ms(0), 1)];

            // unlimitted
            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (1, ms(0)), (0, ms(0)), (1, ms(0))];
            future::join(produce(tx, input), verify(into_stream(rx), expected)).await;

            // debounced
            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(1000)), (1, ms(1000))];
            future::join(
                produce(tx, input),
                verify(RateLimit::debounce(into_stream(rx), ms(1000)), expected),
            )
            .await;
        }

        fn ms(ms: u64) -> Duration {
            Duration::from_millis(ms)
        }

        fn into_stream<T>(rx: mpsc::Receiver<T>) -> impl Stream<Item = T> {
            stream::unfold(rx, |mut rx| async move { Some((rx.recv().await?, rx)) })
        }

        async fn produce<T>(tx: mpsc::Sender<T>, input: impl IntoIterator<Item = (Duration, T)>) {
            for (delay, item) in input {
                time::sleep(delay).await;
                tx.send(item).await.ok().unwrap();
            }
        }

        async fn verify<T: Eq + Debug>(
            stream: impl Stream<Item = T>,
            expected: impl IntoIterator<Item = (T, Duration)>,
        ) {
            let start = Instant::now();
            let actual: Vec<_> = stream.map(|item| (item, start.elapsed())).collect().await;
            let expected: Vec<_> = expected.into_iter().collect();

            assert_eq!(actual, expected);
        }
    }
}

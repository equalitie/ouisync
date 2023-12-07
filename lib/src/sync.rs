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

        pub fn is_closed(&self) -> bool {
            // `tokio::sync::watch::Receiver` doesn't expose a `is_closed`, but we can get the same
            // information by checking whether `has_changed` returns an error as it only does so
            // when the chanel has been closed.
            self.0.has_changed().is_err()
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

        pub fn is_closed(&self) -> bool {
            self.watch_rx.is_closed()
        }
    }

    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            self.shared.receivers.lock().unwrap().remove(&self.id);
        }
    }

    type ReceiverState<T> = (uninitialized_watch::Sender<()>, HashSet<T>);

    struct Shared<T> {
        id_generator: AtomicUsize,
        receivers: Mutex<HashMap<usize, ReceiverState<T>>>,
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
    use pin_project_lite::pin_project;
    use std::{
        collections::{hash_map, BTreeMap, HashMap},
        fmt::Debug,
        future::Future,
        hash::Hash,
        pin::Pin,
        task::{ready, Context, Poll},
    };
    use tokio::time::{self, Duration, Instant, Sleep};

    type EntryId = u64;

    struct Delay {
        until: Instant,
        next: Option<EntryId>,
    }

    pin_project! {
        /// Rate-limitting stream adapter.
        ///
        /// ```ignore
        /// in:       |a|a|a|a|a| | | | | |a|a|a|a| | | |
        /// throttle: |a| |a| |a| | | | | |a| |a| |a| | |
        /// ```
        ///
        /// Multiple occurences of the same item within the rate-limit period are reduced to a
        /// single one but distinct items are preserved:
        ///
        /// ```ignore
        /// in:        |a|b|a|b| | | | |
        /// throttle:  |a| |a|b| | | | |
        /// ```
        pub struct Throttle<S>
        where
            S: Stream,
            S::Item: Hash,
        {
            #[pin]
            inner: Fuse<S>,
            period: Duration,
            ready: BTreeMap<EntryId, S::Item>,
            delays: HashMap<S::Item, Delay>,
            #[pin]
            sleep: Option<Sleep>,
            next_id: EntryId,
        }
    }

    impl<S> Throttle<S>
    where
        S: Stream,
        S::Item: Eq + Hash,
    {
        pub fn new(inner: S, period: Duration) -> Self {
            Self {
                inner: inner.fuse(),
                period,
                ready: BTreeMap::new(),
                delays: HashMap::new(),
                sleep: None,
                next_id: 0,
            }
        }

        fn is_ready(ready: &BTreeMap<EntryId, S::Item>, item: &S::Item) -> bool {
            for v in ready.values() {
                if v == item {
                    return true;
                }
            }
            false
        }
    }

    impl<S> Stream for Throttle<S>
    where
        S: Stream,
        S::Item: Hash + Eq + Clone + Debug,
    {
        type Item = S::Item;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let mut this = self.project();
            let now = Instant::now();
            let mut inner_is_finished = false;

            // Take entries from `inner` into `items` ASAP so we can timestamp them.
            loop {
                match this.inner.as_mut().poll_next(cx) {
                    Poll::Ready(Some(item)) => {
                        let is_ready = Self::is_ready(this.ready, &item);

                        match this.delays.entry(item.clone()) {
                            hash_map::Entry::Occupied(mut entry) => {
                                if !is_ready {
                                    let delay = entry.get_mut();

                                    if delay.next.is_none() {
                                        let entry_id = *this.next_id;
                                        *this.next_id += 1;
                                        delay.next = Some(entry_id);
                                    }
                                }
                            }
                            hash_map::Entry::Vacant(entry) => {
                                let entry_id = *this.next_id;
                                *this.next_id += 1;

                                if !is_ready {
                                    this.ready.insert(entry_id, item.clone());
                                }

                                entry.insert(Delay {
                                    until: now + *this.period,
                                    next: None,
                                });
                            }
                        }
                    }
                    Poll::Ready(None) => {
                        inner_is_finished = true;
                        break;
                    }
                    Poll::Pending => {
                        break;
                    }
                }
            }

            loop {
                if let Some(first_entry) = this.ready.first_entry() {
                    return Poll::Ready(Some(first_entry.remove()));
                }

                if let Some(sleep) = this.sleep.as_mut().as_pin_mut() {
                    ready!(sleep.poll(cx));
                    this.sleep.set(None);
                }

                let mut first: Option<(&S::Item, &mut Delay)> = None;

                for (item, delay) in this.delays.iter_mut() {
                    if let Some((_, first_delay)) = &first {
                        if (delay.until, delay.next) < (first_delay.until, first_delay.next) {
                            first = Some((item, delay));
                        }
                    } else {
                        first = Some((item, delay));
                    }
                }

                let (first_item, first_delay) = match &mut first {
                    Some(first) => (&first.0, &mut first.1),
                    None => {
                        return if inner_is_finished {
                            Poll::Ready(None)
                        } else {
                            Poll::Pending
                        }
                    }
                };

                if first_delay.until <= now {
                    let first_item = (*first_item).clone();

                    if first_delay.next.is_some() {
                        first_delay.until = now + *this.period;
                        first_delay.next = None;
                        return Poll::Ready(Some(first_item));
                    } else {
                        this.delays.remove(&first_item);
                    }
                } else {
                    this.sleep.set(Some(time::sleep_until(first_delay.until)));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use futures_util::{future, stream, StreamExt};
        use std::fmt::Debug;
        use tokio::{sync::mpsc, time::Instant};

        #[tokio::test(start_paused = true)]
        async fn rate_limit_equal_items() {
            let input = [(ms(0), 0), (ms(0), 0)];
            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (0, ms(1000))];
            future::join(
                produce(tx, input),
                verify(Throttle::new(into_stream(rx), ms(1000)), expected),
            )
            .await;

            //--------------------------------------------------------

            let input = [(ms(0), 0), (ms(100), 0)];
            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (0, ms(1000))];
            future::join(
                produce(tx, input),
                verify(Throttle::new(into_stream(rx), ms(1000)), expected),
            )
            .await;

            //--------------------------------------------------------

            let input = [(ms(0), 0), (ms(0), 0), (ms(1001), 0)];
            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (0, ms(1000)), (0, ms(2000))];
            future::join(
                produce(tx, input),
                verify(Throttle::new(into_stream(rx), ms(1000)), expected),
            )
            .await;

            //--------------------------------------------------------

            let input = [
                (ms(0), 0),
                (ms(100), 0),
                (ms(100), 0),
                (ms(1500), 0),
                (ms(0), 0),
            ];

            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (0, ms(1000)), (0, ms(2000))];
            future::join(
                produce(tx, input),
                verify(Throttle::new(into_stream(rx), ms(1000)), expected),
            )
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn rate_limit_inequal_items() {
            let input = [(ms(0), 0), (ms(0), 1)];

            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (1, ms(0))];
            future::join(
                produce(tx, input),
                verify(Throttle::new(into_stream(rx), ms(1000)), expected),
            )
            .await;

            //--------------------------------------------------------

            let input = [(ms(0), 0), (ms(0), 1), (ms(0), 0), (ms(0), 1)];

            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (1, ms(0)), (0, ms(1000)), (1, ms(1000))];
            future::join(
                produce(tx, input),
                verify(Throttle::new(into_stream(rx), ms(1000)), expected),
            )
            .await;

            //--------------------------------------------------------

            let input = [(ms(0), 0), (ms(0), 1), (ms(0), 1), (ms(0), 0)];

            let (tx, rx) = mpsc::channel(1);
            let expected = [(0, ms(0)), (1, ms(0)), (1, ms(1000)), (0, ms(1000))];
            future::join(
                produce(tx, input),
                verify(Throttle::new(into_stream(rx), ms(1000)), expected),
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

/// A hash map whose entries expire after a timeout.
pub(crate) mod delay_map {
    use crate::collections::{hash_map, HashMap};
    use std::{
        borrow::Borrow,
        hash::Hash,
        task::{ready, Context, Poll},
    };
    use tokio::time::Duration;
    use tokio_util::time::delay_queue::{DelayQueue, Key};

    pub struct DelayMap<K, V> {
        items: HashMap<K, Item<V>>,
        delays: DelayQueue<K>,
    }

    impl<K, V> DelayMap<K, V> {
        pub fn new() -> Self {
            Self {
                items: HashMap::default(),
                delays: DelayQueue::default(),
            }
        }
    }

    impl<K, V> DelayMap<K, V>
    where
        K: Eq + Hash + Clone,
    {
        // This is unused right now but keeping it around so we don't have to recreate when we need
        // it.
        #[allow(unused)]
        pub fn insert(&mut self, key: K, value: V, timeout: Duration) -> Option<V> {
            let delay_key = self.delays.insert(key.clone(), timeout);
            let old = self.items.insert(key, Item { value, delay_key })?;

            self.delays.remove(&old.delay_key);

            Some(old.value)
        }

        pub fn try_insert(&mut self, key: K) -> Option<VacantEntry<K, V>> {
            match self.items.entry(key) {
                hash_map::Entry::Vacant(item_entry) => Some(VacantEntry {
                    item_entry,
                    delays: &mut self.delays,
                }),
                hash_map::Entry::Occupied(_) => None,
            }
        }

        pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
        where
            K: Borrow<Q>,
            Q: Hash + Eq + ?Sized,
        {
            let item = self.items.remove(key)?;
            self.delays.remove(&item.delay_key);

            Some(item.value)
        }

        pub fn drain(&mut self) -> impl Iterator<Item = (K, V)> + '_ {
            self.items.drain().map(|(key, item)| {
                self.delays.remove(&item.delay_key);
                (key, item.value)
            })
        }

        pub fn len(&self) -> usize {
            self.items.len()
        }

        pub fn is_empty(&self) -> bool {
            self.items.is_empty()
        }

        /// Poll for the next expired item. This can be wrapped in `future::poll_fn` and awaited.
        /// Returns `Poll::Ready(None)` if the map is empty.
        pub fn poll_expired(&mut self, cx: &mut Context<'_>) -> Poll<Option<(K, V)>> {
            if let Some(expired) = ready!(self.delays.poll_expired(cx)) {
                let key = expired.into_inner();
                // unwrap is OK because an entry exists in `delays` iff it exists in `items`.
                let item = self.items.remove(&key).unwrap();

                Poll::Ready(Some((key, item.value)))
            } else {
                Poll::Ready(None)
            }
        }
    }

    impl<K, V> Default for DelayMap<K, V> {
        fn default() -> Self {
            Self::new()
        }
    }

    pub struct VacantEntry<'a, K, V> {
        item_entry: hash_map::VacantEntry<'a, K, Item<V>>,
        delays: &'a mut DelayQueue<K>,
    }

    impl<'a, K, V> VacantEntry<'a, K, V>
    where
        K: Clone,
    {
        pub fn insert(self, value: V, timeout: Duration) -> &'a V {
            let delay_key = self.delays.insert(self.item_entry.key().clone(), timeout);
            &mut self.item_entry.insert(Item { value, delay_key }).value
        }
    }

    struct Item<V> {
        value: V,
        delay_key: Key,
    }
}

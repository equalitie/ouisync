use std::{
    borrow::Borrow,
    collections::HashMap,
    hash::Hash,
    pin::Pin,
    task::{Context, Poll},
};

use futures_util::Stream;

/// Similar to
/// [FuturesUnordered](https://docs.rs/futures/latest/futures/stream/struct.FuturesUnordered.html),
/// but each future is associated with a key that can be used to cancel the future before
/// completion.
///
/// Note unlike `FuturesUnordered`, this requires the futures to be `Unpin`. To use non-unpin
/// futures, consider wrapping them in Pin<Box>.
pub struct FuturesMap<K, F> {
    // TODO: this implementation is inneficient if there is a large number of futures because
    // currently every time we poll the map we poll every future in it. More efficient
    // implementation would poll only the futures that have been woken since the last poll.
    futures: HashMap<K, F>,
}

impl<K, F> FuturesMap<K, F> {
    pub fn new() -> Self {
        Self {
            futures: HashMap::new(),
        }
    }
}

impl<K, F> FuturesMap<K, F>
where
    K: Eq + Hash,
{
    /// Inserts a future into the map under the given key.
    ///
    /// Returns whether the future was newly inserted, that is, if the map did not previously
    /// contain a future with the same key, returns `true`, otherwise returns `false` and the
    /// existing future is dropped.
    pub fn insert(&mut self, key: K, future: F) -> bool {
        self.futures.insert(key, future).is_none()
    }

    /// Removes and drops the future associated with the given key.
    ///
    /// Returns whether the future was present in the map.
    pub fn remove<Q>(&mut self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.futures.remove(key).is_some()
    }
}

impl<K, F> Default for FuturesMap<K, F> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, F> Stream for FuturesMap<K, F>
where
    K: Clone + Eq + Hash + Unpin,
    F: Future + Unpin,
{
    type Item = (K, F::Output);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let futures = &mut self.get_mut().futures;

        if futures.is_empty() {
            return Poll::Ready(None);
        }

        for (key, future) in &mut *futures {
            match Pin::new(future).poll(cx) {
                Poll::Ready(value) => {
                    let key = key.clone();
                    futures.remove(&key);
                    return Poll::Ready(Some((key, value)));
                }
                Poll::Pending => (),
            }
        }

        Poll::Pending
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.futures.len()))
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{FutureExt, StreamExt};
    use tokio::sync::oneshot;

    use super::*;

    #[test]
    fn sanity_check() {
        let (tx_0, rx_0) = oneshot::channel::<&str>();
        let (tx_1, rx_1) = oneshot::channel::<&str>();
        let (tx_2, rx_2) = oneshot::channel::<&str>();

        let mut map = FuturesMap::<u32, oneshot::Receiver<&str>>::new();
        assert_eq!(map.next().now_or_never(), Some(None));

        assert!(map.insert(0, rx_0));
        assert!(map.insert(1, rx_1));
        assert!(map.insert(2, rx_2));
        assert_eq!(map.next().now_or_never(), None);

        tx_0.send("zero").unwrap();
        assert_eq!(map.next().now_or_never(), Some(Some((0, Ok("zero")))));

        tx_1.send("one").unwrap();
        tx_2.send("two").unwrap();
        assert!(map.remove(&1));
        assert_eq!(map.next().now_or_never(), Some(Some((2, Ok("two")))));
        assert_eq!(map.next().now_or_never(), Some(None));
    }
}

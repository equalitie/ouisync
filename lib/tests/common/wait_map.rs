use std::{
    collections::{hash_map::Entry, HashMap},
    hash::Hash,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

/// Concurrent hash map that allows asynchronously waiting for a value to become available.
pub(crate) struct WaitMap<K, V> {
    inner: Mutex<HashMap<K, Slot<V>>>,
}

impl<K, V> WaitMap<K, V> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl<K, V> WaitMap<K, V>
where
    K: Eq + Hash,
{
    pub fn insert(&self, key: K, value: V) -> Option<V> {
        let prev = self
            .inner
            .lock()
            .unwrap()
            .insert(key, Slot::Occupied(value));

        match prev {
            Some(Slot::Waiting(notify)) => {
                notify.notify_waiters();
                None
            }
            Some(Slot::Occupied(value)) => Some(value),
            None => None,
        }
    }
}

impl<K, V> WaitMap<K, V>
where
    K: Eq + Hash,
    V: Clone,
{
    /// Returns the value at the given key. If the value is not currently present in the map,
    /// asynchronously waits until it becomes inserted and then returns it.
    pub async fn get<Q>(&self, key: &Q) -> V
    where
        Q: ToOwned<Owned = K> + ?Sized,
    {
        loop {
            let notify = match self.inner.lock().unwrap().entry(key.to_owned()) {
                Entry::Occupied(entry) => match entry.get() {
                    Slot::Occupied(value) => return value.clone(),
                    Slot::Waiting(notify) => notify.clone(),
                },
                Entry::Vacant(entry) => {
                    let notify = Arc::new(Notify::new());
                    entry.insert(Slot::Waiting(notify.clone()));
                    notify
                }
            };

            notify.notified().await;
        }
    }
}

enum Slot<T> {
    Occupied(T),
    Waiting(Arc<Notify>),
}

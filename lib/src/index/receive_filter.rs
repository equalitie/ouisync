use super::node::Summary;
use crate::crypto::Hash;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex as BlockingMutex, MutexGuard as BlockingMutexGuard},
    time::{Duration, Instant},
};

const RECEIVE_FILTER_EXPIRY: Duration = Duration::from_secs(5 * 60);

/// Filter for received nodes to avoid processing a node that doesn't contain any new information
/// compared to the last time we received that same node.
#[derive(Clone)]
pub(super) struct ReceiveFilter {
    inner: Arc<BlockingMutex<Inner>>,
}

impl ReceiveFilter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BlockingMutex::new(Inner {
                entries: HashMap::new(),
                enables: 0,
            })),
        }
    }

    /// Enables the receive filter. The receive filter remains enabled as long as at least one
    /// `Enable` handle to it exists.
    pub fn enable(&self) -> Enable {
        self.inner.lock().unwrap().enables += 1;

        Enable {
            inner: self.inner.clone(),
        }
    }

    /// Obtains a handle to work with the receive filter.
    ///
    /// # Panics
    ///
    /// Panics if the filter is not enabled.
    pub fn access(&self) -> Access {
        let mut inner = self.inner.lock().unwrap();
        assert!(inner.enables > 0);

        inner.clear_expired();

        Access { inner }
    }
}

/// Handle that keeps the receive filter enabled.
pub(crate) struct Enable {
    inner: Arc<BlockingMutex<Inner>>,
}

impl Drop for Enable {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();

        inner.enables -= 1;

        if inner.enables == 0 {
            inner.entries.clear();
        }
    }
}

/// Handle that allows manipulation with the receive filter.
pub(super) struct Access<'a> {
    inner: BlockingMutexGuard<'a, Inner>,
}

impl Access<'_> {
    pub fn check(&mut self, hash: Hash, summary: Summary) -> bool {
        match self.inner.entries.entry(hash) {
            Entry::Occupied(mut entry) => {
                if entry.get().0.is_up_to_date_with(&summary).unwrap_or(false) {
                    false
                } else {
                    entry.insert((summary, Instant::now()));
                    true
                }
            }
            Entry::Vacant(entry) => {
                entry.insert((summary, Instant::now()));
                true
            }
        }
    }
}

struct Inner {
    entries: HashMap<Hash, (Summary, Instant)>,
    enables: usize,
}

impl Inner {
    fn clear_expired(&mut self) {
        self.entries
            .retain(|_, (_, timestamp)| timestamp.elapsed() < RECEIVE_FILTER_EXPIRY)
    }
}

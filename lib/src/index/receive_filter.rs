use super::node::Summary;
use crate::crypto::Hash;
use std::{
    collections::{hash_map::Entry, HashMap},
    time::{Duration, Instant},
};

const RECEIVE_FILTER_EXPIRY: Duration = Duration::from_secs(5 * 60);

/// Filter for received nodes to avoid processing a node that doesn't contain any new information
/// compared to the last time we received that same node.
#[derive(Clone)]
pub(crate) struct ReceiveFilter {
    entries: HashMap<Hash, (Summary, Instant)>,
}

impl ReceiveFilter {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn check(&mut self, hash: Hash, summary: Summary) -> bool {
        match self.entries.entry(hash) {
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

    pub fn clear_expired(&mut self) {
        self.entries
            .retain(|_, (_, timestamp)| timestamp.elapsed() < RECEIVE_FILTER_EXPIRY)
    }
}

use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Tracks the amount of data exchanged with other peers.
#[derive(Default, Clone)]
pub(super) struct TrafficTracker {
    counters: Arc<Counters>,
}

impl TrafficTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_send(&self, bytes: u64) {
        self.counters.send.fetch_add(bytes, Ordering::Release);
    }

    pub fn record_recv(&self, bytes: u64) {
        self.counters.recv.fetch_add(bytes, Ordering::Release);
    }

    pub fn get(&self) -> TrafficStats {
        self.counters.get()
    }
}

/// Network traffic statistics.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct TrafficStats {
    /// Total number of bytes sent from this instance.
    pub send: u64,
    /// Total number of bytes received by this instance.
    pub recv: u64,
}

#[derive(Default)]
struct Counters {
    send: AtomicU64,
    recv: AtomicU64,
}

impl Counters {
    fn get(&self) -> TrafficStats {
        TrafficStats {
            send: self.send.load(Ordering::Acquire),
            recv: self.recv.load(Ordering::Acquire),
        }
    }
}

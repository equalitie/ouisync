//! Innternal state of Blob

use crate::event::{Event, Payload};
use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
};
use tokio::sync::broadcast;

// State shared among multiple instances of the same blob.
pub(crate) struct Shared {
    event_tx: broadcast::Sender<Event>,
    write_lock: AtomicBool,
}

impl Shared {
    pub fn new(event_tx: broadcast::Sender<Event>) -> Arc<Self> {
        Arc::new(Self {
            event_tx,
            write_lock: AtomicBool::new(false),
        })
    }

    pub(super) fn acquire_write_lock(&self) -> bool {
        !self.write_lock.fetch_or(true, Ordering::Relaxed)
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.event_tx
            .send(Event::new(Payload::BlobClosed))
            .unwrap_or(0);
    }
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Shared")
            .field("write_lock", &self.write_lock.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

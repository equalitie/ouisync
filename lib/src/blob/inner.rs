//! Innternal state of Blob

use crate::{
    db,
    event::{Event, Payload},
};
use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
};
use tokio::sync::broadcast;

// State shared among multiple instances of the same blob.
pub(crate) struct Shared {
    event_tx: broadcast::Sender<Event>,
    len_version: AtomicU64,
}

impl Shared {
    pub fn new(event_tx: broadcast::Sender<Event>) -> Arc<Self> {
        Arc::new(Self {
            event_tx,
            len_version: AtomicU64::new(0),
        })
    }

    pub fn len_version(&self) -> u64 {
        self.len_version.load(Ordering::Relaxed)
    }

    // Note this function must be called within a db transaction to make sure the calling task is
    // the only one setting the shared length at any given time.
    pub fn set_len_version(&self, _tx: &mut db::Transaction<'_>, version: u64) {
        self.len_version.store(version, Ordering::Relaxed)
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
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

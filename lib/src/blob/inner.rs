//! Innternal state of Blob

use super::open_block::OpenBlock;
use crate::{
    block::BLOCK_SIZE,
    branch::Branch,
    db,
    event::{Event, Payload},
    locator::Locator,
};
use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
};
use tokio::sync::broadcast;

// State unique to each instance of a blob.
#[derive(Clone)]
pub(super) struct Unique {
    pub branch: Branch,
    pub head_locator: Locator,
    pub current_block: OpenBlock,
    pub len: u64,
    pub len_dirty: bool,
    pub len_version: u64,
}

impl Unique {
    pub fn locator_at(&self, number: u32) -> Locator {
        self.head_locator.nth(number)
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    pub fn block_count(&self) -> u32 {
        block_count(self.len)
    }
}

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

pub(super) fn block_count(len: u64) -> u32 {
    // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
    (1 + (len + super::HEADER_SIZE as u64 - 1) / BLOCK_SIZE as u64)
        .try_into()
        .unwrap_or(u32::MAX)
}

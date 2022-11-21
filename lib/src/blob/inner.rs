//! Innternal state of Blob

use super::open_block::OpenBlock;
use crate::{
    block::BLOCK_SIZE,
    branch::Branch,
    event::{Event, Payload},
    locator::Locator,
};
use std::{fmt, sync::Arc};
use tokio::sync::broadcast;

// State unique to each instance of a blob.
#[derive(Clone)]
pub(super) struct Unique {
    pub branch: Branch,
    pub head_locator: Locator,
    pub current_block: OpenBlock,
    pub len: u64,
    pub len_dirty: bool,
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
}

impl Shared {
    pub fn detached() -> Arc<Self> {
        let (tx, _) = broadcast::channel(1);
        Self::new(tx)
    }

    pub fn new(event_tx: broadcast::Sender<Event>) -> Arc<Self> {
        Arc::new(Self { event_tx })
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

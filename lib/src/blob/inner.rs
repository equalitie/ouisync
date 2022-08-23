//! Innternal state of Blob

use super::open_block::OpenBlock;
use crate::{
    block::BLOCK_SIZE,
    branch::Branch,
    event::{Event, Payload},
    locator::Locator,
    sync::Mutex,
};
use std::{
    fmt,
    sync::{Arc, Weak},
};
use tokio::sync::broadcast;

// State unique to each instance of a blob.
#[derive(Clone)]
pub(super) struct Unique {
    pub branch: Branch,
    pub head_locator: Locator,
    pub current_block: OpenBlock,
    pub len_dirty: bool,
}

// State shared among multiple instances of the same blob.
pub(crate) struct Shared {
    pub(super) len: u64,
    event_tx: broadcast::Sender<Event>,
}

impl Shared {
    pub fn uninit() -> MaybeInitShared {
        let (tx, _) = broadcast::channel(1);
        Self::uninit_with_close_notify(tx)
    }

    pub fn uninit_with_close_notify(tx: broadcast::Sender<Event>) -> MaybeInitShared {
        MaybeInitShared {
            shared: Self::new(0, tx).into_locked(),
            init: false,
        }
    }

    pub fn deep_clone(&self) -> Self {
        Self::new(self.len, self.event_tx.clone())
    }

    pub fn into_locked(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }

    fn new(len: u64, event_tx: broadcast::Sender<Event>) -> Self {
        Self { len, event_tx }
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    pub fn block_count(&self) -> u32 {
        block_count(self.len)
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
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

// Wrapper for `Shared` that may or might not be initialized.
#[derive(Clone)]
pub(crate) struct MaybeInitShared {
    shared: Arc<Mutex<Shared>>,
    init: bool,
}

impl MaybeInitShared {
    pub(super) async fn ensure_init(self, len: u64) -> Arc<Mutex<Shared>> {
        if !self.init {
            self.shared.lock().await.len = len;
        }

        self.shared
    }

    pub(super) fn assume_init(self) -> Arc<Mutex<Shared>> {
        self.shared
    }

    pub(crate) fn downgrade(&self) -> Weak<Mutex<Shared>> {
        Arc::downgrade(&self.shared)
    }
}

impl From<Arc<Mutex<Shared>>> for MaybeInitShared {
    fn from(shared: Arc<Mutex<Shared>>) -> Self {
        Self { shared, init: true }
    }
}

pub(super) fn block_count(len: u64) -> u32 {
    // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
    (1 + (len + super::HEADER_SIZE as u64 - 1) / BLOCK_SIZE as u64)
        .try_into()
        .unwrap_or(u32::MAX)
}

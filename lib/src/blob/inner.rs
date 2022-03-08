//! Innternal state of Blob

use super::OpenBlock;
use crate::{block::BLOCK_SIZE, branch::Branch, locator::Locator};
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;

// State unique to each instance of a blob.
pub(super) struct Unique {
    pub branch: Branch,
    pub head_locator: Locator,
    pub current_block: OpenBlock,
    pub len_dirty: bool,
}

// State shared among multiple instances of the same blob.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(super) len: u64,
}

impl Shared {
    pub fn uninit() -> UninitShared {
        UninitShared(Self::new(0))
    }

    pub fn deep_clone(&self) -> Arc<Mutex<Self>> {
        Self::new(self.len)
    }

    fn new(len: u64) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self { len }))
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    pub fn block_count(&self) -> u32 {
        // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
        (1 + (self.len + super::HEADER_SIZE as u64 - 1) / BLOCK_SIZE as u64)
            .try_into()
            .unwrap_or(u32::MAX)
    }
}

// Wrapper for `Shared` that's not initialized.
#[derive(Clone)]
pub(crate) struct UninitShared(Arc<Mutex<Shared>>);

impl UninitShared {
    pub(super) fn init(self) -> Arc<Mutex<Shared>> {
        self.0
    }

    pub(crate) fn downgrade(&self) -> Weak<Mutex<Shared>> {
        Arc::downgrade(&self.0)
    }
}

// Wrapper for `Shared` that may or might not be initialized.
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
}

impl From<Arc<Mutex<Shared>>> for MaybeInitShared {
    fn from(shared: Arc<Mutex<Shared>>) -> Self {
        Self { shared, init: true }
    }
}

impl From<UninitShared> for MaybeInitShared {
    fn from(shared: UninitShared) -> Self {
        Self {
            shared: shared.0,
            init: false,
        }
    }
}

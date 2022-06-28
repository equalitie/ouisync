use super::inner::{MaybeInitShared, Shared};
use crate::{blob_id::BlobId, crypto::sign::PublicKey, sync::Mutex as AsyncMutex};
use std::{
    collections::HashMap,
    sync::{Mutex as BlockingMutex, Weak},
};

pub(crate) struct BlobCache {
    slots: BlockingMutex<BranchMap>,
}

type BlobMap = HashMap<BlobId, Weak<AsyncMutex<Shared>>>;
type BranchMap = HashMap<PublicKey, BlobMap>;

impl BlobCache {
    pub fn new() -> Self {
        Self {
            slots: BlockingMutex::new(HashMap::new()),
        }
    }

    pub fn fetch(&self, branch_id: PublicKey, blob_id: BlobId) -> MaybeInitShared {
        let mut slots = self.slots.lock().unwrap();

        // Cleanup
        for branch in slots.values_mut() {
            branch.retain(|_, slot| slot.strong_count() > 0);
        }

        slots.retain(|_, branch| !branch.is_empty());

        let slot = slots
            .entry(branch_id)
            .or_default()
            .entry(blob_id)
            .or_insert_with(Weak::new);

        if let Some(shared) = slot.upgrade() {
            shared.into()
        } else {
            let shared = Shared::uninit();
            *slot = shared.downgrade();
            shared
        }
    }

    pub fn contains(&self, branch_id: &PublicKey, blob_id: &BlobId) -> bool {
        self.slots
            .lock()
            .unwrap()
            .get(branch_id)
            .and_then(|branch| branch.get(blob_id))
            .map(|slot| slot.strong_count() > 0)
            .unwrap_or(false)
    }
}

use super::inner::{MaybeInitShared, Shared};
use crate::{blob_id::BlobId, crypto::sign::PublicKey, sync::Mutex as AsyncMutex};
use std::{
    collections::HashMap,
    sync::{Mutex as BlockingMutex, Weak},
};

pub(crate) struct BlobCache {
    slots: BlockingMutex<HashMap<Key, Weak<AsyncMutex<Shared>>>>,
}

impl BlobCache {
    pub fn new() -> Self {
        Self {
            slots: BlockingMutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, branch_id: PublicKey, blob_id: BlobId) -> MaybeInitShared {
        let mut slots = self.slots.lock().unwrap();

        // Cleanup
        slots.retain(|_, slot| slot.strong_count() > 0);

        let slot = slots
            .entry(Key { branch_id, blob_id })
            .or_insert_with(Weak::new);

        if let Some(shared) = slot.upgrade() {
            shared.into()
        } else {
            let shared = Shared::uninit();
            *slot = shared.downgrade();
            shared
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
struct Key {
    branch_id: PublicKey,
    blob_id: BlobId,
}

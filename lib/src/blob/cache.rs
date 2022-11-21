use super::inner::Shared;
use crate::{blob_id::BlobId, crypto::sign::PublicKey, event::Event, sync::Mutex as AsyncMutex};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as BlockingMutex, Weak},
};
use tokio::sync::broadcast;

pub(crate) struct BlobCache {
    slots: BlockingMutex<BranchMap>,
    event_tx: broadcast::Sender<Event>,
}

type BlobMap = HashMap<BlobId, Weak<AsyncMutex<Shared>>>;
type BranchMap = HashMap<PublicKey, BlobMap>;

impl BlobCache {
    pub fn new(event_tx: broadcast::Sender<Event>) -> Self {
        Self {
            slots: BlockingMutex::new(HashMap::new()),
            event_tx,
        }
    }

    pub fn fetch(&self, branch_id: PublicKey, blob_id: BlobId) -> Arc<AsyncMutex<Shared>> {
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
            shared
        } else {
            let shared = Shared::new(self.event_tx.clone());
            *slot = Arc::downgrade(&shared);
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

    pub fn contains_any(&self, branch_id: &PublicKey) -> bool {
        self.slots
            .lock()
            .unwrap()
            .get(branch_id)
            .map(|branch| branch.values().any(|slot| slot.strong_count() > 0))
            .unwrap_or(false)
    }
}

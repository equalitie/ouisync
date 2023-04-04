use crate::{
    blob_id::BlobId,
    collections::{hash_map::Entry, HashMap},
    crypto::sign::PublicKey,
    deadlock::blocking::Mutex,
};
use std::sync::Arc;

/// Protects blob from being garbage collected.
pub(crate) struct BlobPin {
    shared: Arc<Mutex<Shared>>,
    branch_id: PublicKey,
    blob_id: BlobId,
}

impl Clone for BlobPin {
    fn clone(&self) -> Self {
        let mut shared = self.shared.lock().unwrap();

        // unwraps are ok because the entries exist as long as self exists.
        *shared
            .get_mut(&self.branch_id)
            .unwrap()
            .get_mut(&self.blob_id)
            .unwrap() += 1;

        Self {
            shared: self.shared.clone(),
            branch_id: self.branch_id,
            blob_id: self.blob_id,
        }
    }
}

impl Drop for BlobPin {
    fn drop(&mut self) {
        let mut shared = self.shared.lock().unwrap();

        match shared.entry(self.branch_id) {
            Entry::Occupied(mut branch_entry) => {
                match branch_entry.get_mut().entry(self.blob_id) {
                    Entry::Occupied(mut blob_entry) => {
                        *blob_entry.get_mut() -= 1;

                        if *blob_entry.get() == 0 {
                            blob_entry.remove();
                        }
                    }
                    Entry::Vacant(_) => unreachable!(),
                }

                if branch_entry.get().is_empty() {
                    branch_entry.remove();
                }
            }
            Entry::Vacant(_) => unreachable!(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct BlobPinner {
    shared: Arc<Mutex<Shared>>,
}

type Shared = HashMap<PublicKey, HashMap<BlobId, usize>>;

impl BlobPinner {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Creates pin for the blob with the given id. The pin is released when dropped.
    pub fn pin(&self, branch_id: PublicKey, blob_id: BlobId) -> BlobPin {
        let mut shared = self.shared.lock().unwrap();

        let blobs = shared.entry(branch_id).or_default();
        *blobs.entry(blob_id).or_insert(0) += 1;

        BlobPin {
            shared: self.shared.clone(),
            branch_id,
            blob_id,
        }
    }

    /// Returns whether the given blob is currently pinned
    pub fn is_pinned(&self, branch_id: &PublicKey, blob_id: &BlobId) -> bool {
        self.shared
            .lock()
            .unwrap()
            .get(branch_id)
            .map(|blobs| blobs.contains_key(blob_id))
            .unwrap_or(false)
    }

    /// Returns the ids of all currently pinned blobs grouped by their branch id.
    pub fn all(&self) -> Vec<(PublicKey, Vec<BlobId>)> {
        self.shared
            .lock()
            .unwrap()
            .iter()
            .map(|(branch_id, blobs)| (*branch_id, blobs.keys().copied().collect()))
            .collect()
    }
}

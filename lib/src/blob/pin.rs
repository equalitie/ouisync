use super::BlobId;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex},
};

/// Creating a blob pin protects the blob's blocks from being garbage collected as long as the pin
/// exists. This is useful when we want to preserve a blob that's not (yet) referenced from any
/// directory in the repository.
pub(crate) struct BlobPin {
    set: Arc<BlobPinSet>,
    id: BlobId,
}

impl Drop for BlobPin {
    fn drop(&mut self) {
        let mut inner = self.set.inner.lock().unwrap();
        match inner.entry(self.id) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() -= 1;

                if *entry.get() == 0 {
                    entry.remove();
                }
            }
            Entry::Vacant(_) => unreachable!(),
        }
    }
}

pub(crate) struct BlobPinSet {
    inner: Mutex<HashMap<BlobId, usize>>,
}

impl BlobPinSet {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Acquires pin for the blob with the given id. The pin is released when dropped.
    pub fn acquire(self: &Arc<Self>, id: BlobId) -> BlobPin {
        let mut inner = self.inner.lock().unwrap();
        *inner.entry(id).or_insert(0) += 1;

        BlobPin {
            set: self.clone(),
            id,
        }
    }

    /// Returns the ids of all currently pinned blobs.
    pub fn all(&self) -> Vec<BlobId> {
        self.inner.lock().unwrap().keys().copied().collect()
    }
}

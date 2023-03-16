use super::BlobId;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

/// Creating a blob pin protects the blob's blocks from being garbage collected as long as the pin
/// exists. This is useful when we want to preserve a blob that's not (yet) referenced from any
/// directory in the repository.
pub(crate) struct BlobPin {
    pinner: Arc<BlobPinnerShared>,
    id: BlobId,
}

impl Drop for BlobPin {
    fn drop(&mut self) {
        let mut pinner = self.pinner.lock().unwrap();
        match pinner.entry(self.id) {
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

#[derive(Clone)]
pub(crate) struct BlobPinner {
    inner: Arc<BlobPinnerShared>,
}

type BlobPinnerShared = Mutex<HashMap<BlobId, usize>>;

impl BlobPinner {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Creates pin for the blob with the given id. The pin is released when dropped.
    pub fn pin(&self, id: BlobId) -> BlobPin {
        let mut inner = self.inner.lock().unwrap();
        *inner.entry(id).or_insert(0) += 1;

        BlobPin {
            pinner: self.inner.clone(),
            id,
        }
    }

    /// Returns the ids of all currently pinned blobs.
    pub fn all(&self) -> Vec<BlobId> {
        self.inner.lock().unwrap().keys().copied().collect()
    }
}

#[derive(Clone)]
pub(crate) struct BranchPinner {
    lock: Arc<RwLock<()>>,
}

impl BranchPinner {
    pub fn new() -> Self {
        Self {
            lock: Arc::new(RwLock::new(())),
        }
    }

    pub async fn pin(&self) -> BranchPin {
        BranchPin(Arc::new(self.lock.clone().read_owned().await))
    }

    pub fn try_prune(&self) -> Option<PruneGuard> {
        self.lock.clone().try_write_owned().ok().map(PruneGuard)
    }
}

// NOTE: Why is `OwnerRwLockReadGuard` not clone?
#[derive(Clone)]
pub(crate) struct BranchPin(Arc<OwnedRwLockReadGuard<()>>);

pub(crate) struct PruneGuard(OwnedRwLockWriteGuard<()>);

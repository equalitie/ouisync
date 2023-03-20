//! Pinning (not to confuse with `std::pin::Pin`) is a mechanism to protect certain objects from
//! being garbage collected. Normally the garbage collector only collects objects it considers
//! unneeded (outdated, unreachable, etc...) but there are cases where some objects would normally
//! be considered as such, but we still don't want to gc them for various reasons.

/// Blob pin prevents blob from being gc-ed before it is inserted into its destination directory.
pub(crate) mod blob {
    use crate::blob_id::BlobId;
    use std::{
        collections::{hash_map::Entry, HashMap},
        sync::{Arc, Mutex},
    };

    pub(crate) struct BlobPin {
        pinner: Arc<Shared>,
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
        inner: Arc<Shared>,
    }

    type Shared = Mutex<HashMap<BlobId, usize>>;

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
}

/// Branch pin prevents outdated branch from being pruned if there are still files and/or
/// directories from that branch that are being accessed.
pub(crate) mod branch {
    use std::sync::Arc;
    use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

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
}

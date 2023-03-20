//! Pinning (not to confuse with `std::pin::Pin`) is a mechanism to protect certain objects from
//! being garbage collected. Normally the garbage collector only collects objects it considers
//! unneeded (outdated, unreachable, etc...) but there are cases where some objects would normally
//! be considered as such, but we still don't want to gc them for various reasons.

/// Blob pin prevents blob from being gc-ed before it is inserted into its destination directory.
pub(crate) mod blob {
    use crate::{
        blob_id::BlobId,
        collections::{hash_map::Entry, HashMap},
    };
    use std::sync::{Arc, Mutex};

    pub(crate) struct BlobPin {
        shared: Arc<Mutex<Shared>>,
        id: BlobId,
    }

    impl Drop for BlobPin {
        fn drop(&mut self) {
            let mut pinner = self.shared.lock().unwrap();
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
        shared: Arc<Mutex<Shared>>,
    }

    type Shared = HashMap<BlobId, usize>;

    impl BlobPinner {
        pub fn new() -> Self {
            Self {
                shared: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        /// Creates pin for the blob with the given id. The pin is released when dropped.
        pub fn pin(&self, id: BlobId) -> BlobPin {
            let mut inner = self.shared.lock().unwrap();
            *inner.entry(id).or_insert(0) += 1;

            BlobPin {
                shared: self.shared.clone(),
                id,
            }
        }

        /// Returns the ids of all currently pinned blobs.
        pub fn all(&self) -> Vec<BlobId> {
            self.shared.lock().unwrap().keys().copied().collect()
        }
    }
}

/// Branch pin prevents outdated branch from being pruned if any files and/or directories from that
/// branch are currently being accessed.
pub(crate) mod branch {
    use crate::{
        collections::{hash_map::Entry, HashMap},
        crypto::sign::PublicKey,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub(crate) struct BranchPinner {
        shared: Arc<Mutex<Shared>>,
    }

    struct Shared {
        loads: usize,
        branches: HashMap<PublicKey, State>,
    }

    enum State {
        Pinned(usize),
        Pruned,
    }

    impl BranchPinner {
        pub fn new() -> Self {
            Self {
                shared: Arc::new(Mutex::new(Shared {
                    loads: 0,
                    branches: HashMap::new(),
                })),
            }
        }

        /// Pin the branch to prevent it from being pruned. If this returns `Some` then any
        /// subsequent call to `prune` for the same branch id returns `None`. Returns `None` if
        /// the branch has already been marked for pruning.
        pub fn pin(&self, id: PublicKey) -> Option<BranchPin> {
            let mut shared = self.shared.lock().unwrap();

            match shared.branches.entry(id).or_insert(State::Pinned(0)) {
                State::Pruned => None,
                State::Pinned(count) => {
                    *count += 1;

                    Some(BranchPin {
                        shared: self.shared.clone(),
                        id,
                    })
                }
            }
        }

        /// Acquire the load guard. Do this before loading branches from the db to prevent any branch
        /// from being pruned after being loaded but before being pinned.
        pub fn load(&self) -> LoadGuard {
            self.shared.lock().unwrap().loads += 1;

            LoadGuard {
                shared: self.shared.clone(),
            }
        }

        /// Mark the given branch as being pruned. If this returns `Some` then any subsequent call
        /// to `pin` for the same branch id returns `None` until the `PruneGuard` goes out of
        /// scope. Returns `None` if the given branch has already been pinned or if a load guard
        /// has been acquired.
        pub fn prune(&self, id: PublicKey) -> Option<PruneGuard> {
            let mut shared = self.shared.lock().unwrap();

            if shared.loads > 0 {
                return None;
            }

            match shared.branches.entry(id).or_insert(State::Pruned) {
                State::Pruned => Some(PruneGuard {
                    shared: self.shared.clone(),
                    id,
                }),
                State::Pinned(_) => None,
            }
        }
    }

    pub(crate) struct BranchPin {
        shared: Arc<Mutex<Shared>>,
        id: PublicKey,
    }

    impl Clone for BranchPin {
        fn clone(&self) -> Self {
            let mut shared = self.shared.lock().unwrap();

            match shared.branches.get_mut(&self.id) {
                Some(State::Pinned(count)) => {
                    *count += 1;
                }
                Some(State::Pruned) | None => unreachable!(),
            }

            Self {
                shared: self.shared.clone(),
                id: self.id,
            }
        }
    }

    impl Drop for BranchPin {
        fn drop(&mut self) {
            let mut shared = self.shared.lock().unwrap();

            let Entry::Occupied(mut entry) = shared.branches.entry(self.id) else {
                unreachable!()
            };

            let State::Pinned(count) = entry.get_mut() else {
                unreachable!()
            };

            *count -= 1;

            if *count == 0 {
                entry.remove();
            }
        }
    }

    pub(crate) struct LoadGuard {
        shared: Arc<Mutex<Shared>>,
    }

    impl Drop for LoadGuard {
        fn drop(&mut self) {
            self.shared.lock().unwrap().loads -= 1;
        }
    }

    pub(crate) struct PruneGuard {
        shared: Arc<Mutex<Shared>>,
        id: PublicKey,
    }

    impl Drop for PruneGuard {
        fn drop(&mut self) {
            match self.shared.lock().unwrap().branches.remove(&self.id) {
                Some(State::Pruned) => (),
                Some(State::Pinned(_)) | None => unreachable!(),
            }
        }
    }
}

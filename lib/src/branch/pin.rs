//! Prevents outdated branch from being pruned if any files and/or directories from that branch are
//! currently being accessed.
use crate::{
    collections::{hash_map::Entry, HashMap},
    crypto::sign::PublicKey,
    deadlock::BlockingMutex,
};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct BranchPinner {
    shared: Arc<BlockingMutex<Shared>>,
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
            shared: Arc::new(BlockingMutex::new(Shared {
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
    shared: Arc<BlockingMutex<Shared>>,
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
    shared: Arc<BlockingMutex<Shared>>,
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        self.shared.lock().unwrap().loads -= 1;
    }
}

pub(crate) struct PruneGuard {
    shared: Arc<BlockingMutex<Shared>>,
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

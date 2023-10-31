//! Locks for coordinating concurrent operations on blobs.

use crate::{
    blob::BlobId,
    collections::{hash_map::Entry, HashMap},
    crypto::sign::PublicKey,
    deadlock::BlockingMutex,
    sync::{AwaitDrop, DropAwaitable},
};
use std::sync::Arc;

/// Container for blob locks in all branches.
#[derive(Default, Clone)]
pub(crate) struct Locker {
    shared: Arc<Shared>,
}

type Shared = BlockingMutex<HashMap<PublicKey, HashMap<BlobId, State>>>;

impl Locker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Obtain a locker bound to the given branch.
    pub fn branch(&self, branch_id: PublicKey) -> BranchLocker {
        BranchLocker {
            shared: self.shared.clone(),
            branch_id,
        }
    }

    /// Returns the blob_ids and unlock notifiers of all currently held locks, grouped by their
    /// branch id.
    pub fn all(&self) -> Vec<(PublicKey, Vec<(BlobId, AwaitDrop)>)> {
        self.shared
            .lock()
            .unwrap()
            .iter()
            .map(|(branch_id, states)| {
                (
                    *branch_id,
                    states
                        .iter()
                        .map(|(blob_id, state)| (*blob_id, state.notify.subscribe()))
                        .collect(),
                )
            })
            .collect()
    }
}

/// Container for blob locks in a given branch.
pub(crate) struct BranchLocker {
    shared: Arc<Shared>,
    branch_id: PublicKey,
}

impl BranchLocker {
    /// Try to acquire a read lock. Fails only if a unique lock is currently being held by the
    /// given blob. The error contains a notifier so the caller can decide whether to wait for the
    /// lock to be released or fail immediately.
    pub fn try_read(&self, blob_id: BlobId) -> Result<ReadLock, AwaitDrop> {
        let mut shared = self.shared.lock().unwrap();

        let state = shared
            .entry(self.branch_id)
            .or_default()
            .entry(blob_id)
            .or_insert(State::new(Kind::Read(0)));

        match &mut state.kind {
            Kind::Read(count) | Kind::Write(count) => {
                *count = count.checked_add(1).expect("lock limit reached");

                Ok(ReadLock {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id,
                })
            }
            Kind::Unique => Err(state.notify.subscribe()),
        }
    }

    /// Acquire a read lock, waiting for a unique lock (if any) to be released first.
    pub async fn read(&self, blob_id: BlobId) -> ReadLock {
        loop {
            match self.try_read(blob_id) {
                Ok(lock) => return lock,
                Err(notify) => notify.await,
            }
        }
    }

    /// Try to acquire a unique lock. Fails if any lock is currently being held by the given blob.
    /// The error contains an unlock notifier and the kind of lock currently being held. The caller
    /// can use them to decide whether they want to wait for the lock to be unlocked or fail
    /// immediately.
    pub fn try_unique(&self, blob_id: BlobId) -> Result<UniqueLock, (AwaitDrop, LockKind)> {
        let mut shared = self.shared.lock().unwrap();

        match shared.entry(self.branch_id).or_default().entry(blob_id) {
            Entry::Vacant(entry) => {
                entry.insert(State::new(Kind::Unique));

                Ok(UniqueLock {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id,
                })
            }
            Entry::Occupied(mut entry) => {
                let kind = match entry.get().kind {
                    Kind::Read(_) => LockKind::Read,
                    Kind::Write(_) => LockKind::Write,
                    Kind::Unique => LockKind::Unique,
                };

                let notify = entry.get_mut().notify.subscribe();

                Err((notify, kind))
            }
        }
    }
}

/// Lock that signals that the blob is being read. It protects the blob from being removed.
pub(crate) struct ReadLock {
    shared: Arc<Shared>,
    branch_id: PublicKey,
    blob_id: BlobId,
}

impl ReadLock {
    pub fn blob_id(&self) -> &BlobId {
        &self.blob_id
    }

    pub fn upgrade(&self) -> Option<WriteLock> {
        let mut shared = self.shared.lock().unwrap();

        let Some(state) = shared
            .get_mut(&self.branch_id)
            .and_then(|states| states.get_mut(&self.blob_id))
        else {
            unreachable!();
        };

        match &mut state.kind {
            Kind::Read(count) => {
                state.kind = Kind::Write(count.checked_add(1).expect("lock count limit exceeded"));

                Some(WriteLock {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id: self.blob_id,
                })
            }
            Kind::Write(_) => None,
            Kind::Unique => unreachable!(),
        }
    }
}

impl Clone for ReadLock {
    fn clone(&self) -> Self {
        let mut shared = self.shared.lock().unwrap();

        let Some(state) = shared
            .get_mut(&self.branch_id)
            .and_then(|states| states.get_mut(&self.blob_id))
        else {
            unreachable!();
        };

        match &mut state.kind {
            Kind::Read(count) | Kind::Write(count) => {
                *count = count.checked_add(1).expect("lock count limit exceeded");

                Self {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id: self.blob_id,
                }
            }
            Kind::Unique => unreachable!(),
        }
    }
}

impl Drop for ReadLock {
    fn drop(&mut self) {
        let mut shared = self.shared.lock().unwrap();

        let Entry::Occupied(mut states_entry) = shared.entry(self.branch_id) else {
            unreachable!();
        };

        let Entry::Occupied(mut state_entry) = states_entry.get_mut().entry(self.blob_id) else {
            unreachable!();
        };

        match &mut state_entry.get_mut().kind {
            Kind::Read(count) | Kind::Write(count) => {
                *count = count.checked_sub(1).expect("lock count cannot be zero");

                if *count == 0 {
                    state_entry.remove();
                }
            }
            Kind::Unique => unreachable!(),
        }

        if states_entry.get().is_empty() {
            states_entry.remove();
        }
    }
}

/// Lock that signals that the blob is being written to. It protects the blob from being removed
/// (same as read lock) and additionally protects it from being written to by anyone else.
pub(crate) struct WriteLock {
    shared: Arc<Shared>,
    branch_id: PublicKey,
    blob_id: BlobId,
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        let mut shared = self.shared.lock().unwrap();

        let Entry::Occupied(mut states_entry) = shared.entry(self.branch_id) else {
            unreachable!();
        };

        let Entry::Occupied(mut state_entry) = states_entry.get_mut().entry(self.blob_id) else {
            unreachable!();
        };

        match &mut state_entry.get_mut().kind {
            Kind::Write(count) => {
                *count = count.checked_sub(1).expect("lock count cannot be zero");

                if *count > 0 {
                    state_entry.get_mut().kind = Kind::Read(*count);
                } else {
                    state_entry.remove();
                }
            }
            Kind::Read(_) | Kind::Unique => unreachable!(),
        }

        if states_entry.get().is_empty() {
            states_entry.remove();
        }
    }
}

/// Lock that expresses unique (exclusive) access to a blob. No one else except the owner of the
/// lock can access the blob in any way while this lock is held.
pub(crate) struct UniqueLock {
    shared: Arc<Shared>,
    branch_id: PublicKey,
    blob_id: BlobId,
}

impl Drop for UniqueLock {
    fn drop(&mut self) {
        let mut shared = self.shared.lock().unwrap();

        let Entry::Occupied(mut states_entry) = shared.entry(self.branch_id) else {
            unreachable!();
        };

        let Entry::Occupied(mut state_entry) = states_entry.get_mut().entry(self.blob_id) else {
            unreachable!();
        };

        match &mut state_entry.get_mut().kind {
            Kind::Unique => {
                state_entry.remove();
            }
            Kind::Read(_) | Kind::Write(_) => unreachable!(),
        }

        if states_entry.get().is_empty() {
            states_entry.remove();
        }
    }
}

/// Lock that can be upgraded from read to write.
pub(crate) enum UpgradableLock {
    Read(ReadLock),
    Write(WriteLock),
}

impl UpgradableLock {
    pub fn upgrade(&mut self) -> bool {
        match self {
            Self::Read(lock) => {
                if let Some(lock) = lock.upgrade() {
                    *self = Self::Write(lock);
                    true
                } else {
                    false
                }
            }
            Self::Write(_) => true,
        }
    }
}

/// Type of the lock currently being held for some blob.
pub(crate) enum LockKind {
    Read,
    Write,
    Unique,
}

struct State {
    kind: Kind,
    notify: DropAwaitable,
}

impl State {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            notify: DropAwaitable::new(),
        }
    }
}

#[derive(Clone)]
enum Kind {
    Read(usize),
    Write(usize),
    Unique,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanity_check() {
        let branch_id = PublicKey::random();
        let blob_id: BlobId = rand::random();

        let locker = Locker::new();
        let locker = locker.branch(branch_id);

        let read0 = locker.try_read(blob_id).ok().unwrap();
        let read1 = locker.try_read(blob_id).ok().unwrap();
        let read2 = read0.clone();

        let write0 = read0.upgrade().unwrap();
        assert!(read0.upgrade().is_none());

        drop(write0);
        let write1 = read0.upgrade().unwrap();

        assert!(locker.try_unique(blob_id).is_err());

        drop(write1);
        assert!(locker.try_unique(blob_id).is_err());

        drop(read2);
        assert!(locker.try_unique(blob_id).is_err());

        drop(read1);
        assert!(locker.try_unique(blob_id).is_err());

        drop(read0);
        let remove0 = locker.try_unique(blob_id).ok().unwrap();

        assert!(locker.try_read(blob_id).is_err());
        assert!(locker.try_unique(blob_id).is_err());

        drop(remove0);
        let remove1 = locker.try_unique(blob_id).ok().unwrap();

        drop(remove1);
        let _read3 = locker.try_read(blob_id).ok().unwrap();
    }
}

//! Locks for coordinating concurrent operations on blobs.

use crate::{
    blob_id::BlobId,
    collections::{hash_map::Entry, HashMap},
    crypto::sign::PublicKey,
    deadlock::BlockingMutex,
};
use std::sync::Arc;
use tokio::sync::Notify;

/// Container for blob locks in all branches.
#[derive(Default, Clone)]
pub(crate) struct Locker {
    shared: Arc<Shared>,
}

type Shared = BlockingMutex<HashMap<PublicKey, HashMap<BlobId, State>>>;

#[derive(Clone)]
enum State {
    Read(usize),
    Write(usize),
    Unique(Arc<Notify>),
}

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

    /// Returns the ids of all currently locked blobs grouped by their branch id.
    pub fn all(&self) -> Vec<(PublicKey, Vec<BlobId>)> {
        self.shared
            .lock()
            .unwrap()
            .iter()
            .map(|(branch_id, states)| (*branch_id, states.keys().copied().collect()))
            .collect()
    }
}

/// Container for blob locks in a given branch.
pub(crate) struct BranchLocker {
    shared: Arc<Shared>,
    branch_id: PublicKey,
}

impl BranchLocker {
    /// Try to acquire a read lock. Fails only is a unique lock is currently being held by the
    /// given blob.
    pub fn read(&self, blob_id: BlobId) -> Option<ReadLock> {
        self.try_read(blob_id).ok()
    }

    pub async fn read_wait(&self, blob_id: BlobId) -> ReadLock {
        loop {
            match self.try_read(blob_id) {
                Ok(lock) => return lock,
                Err(notify) => notify.notified().await,
            }
        }
    }

    fn try_read(&self, blob_id: BlobId) -> Result<ReadLock, Arc<Notify>> {
        let mut shared = self.shared.lock().unwrap();

        match shared
            .entry(self.branch_id)
            .or_default()
            .entry(blob_id)
            .or_insert(State::Read(0))
        {
            State::Read(count) | State::Write(count) => {
                *count = count.checked_add(1).expect("lock limit reached");

                Ok(ReadLock {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id,
                })
            }
            State::Unique(notify) => Err(notify.clone()),
        }
    }

    /// Try to acquire a unique lock. Fails if any kind of lock is currently held for the given
    /// blob.
    pub fn unique(&self, blob_id: BlobId) -> Option<UniqueLock> {
        self.try_unique(blob_id).ok()
    }

    /// Try to acquire a unique lock. If another unique lock is currently held for the given blob,
    /// asynchronously waits until it's released. Fails if any other kind of lock is held.
    pub async fn unique_wait(&self, blob_id: BlobId) -> Option<UniqueLock> {
        loop {
            match self.try_unique(blob_id) {
                Ok(lock) => return Some(lock),
                Err(State::Unique(notify)) => notify.notified().await,
                Err(State::Read(_) | State::Write(_)) => return None,
            }
        }
    }

    fn try_unique(&self, blob_id: BlobId) -> Result<UniqueLock, State> {
        let mut shared = self.shared.lock().unwrap();

        match shared.entry(self.branch_id).or_default().entry(blob_id) {
            Entry::Vacant(entry) => {
                entry.insert(State::Unique(Arc::new(Notify::new())));

                Ok(UniqueLock {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id,
                })
            }
            Entry::Occupied(entry) => Err(entry.get().clone()),
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
    pub fn upgrade(&self) -> Option<WriteLock> {
        let mut shared = self.shared.lock().unwrap();

        let Some(state) = shared
            .get_mut(&self.branch_id)
            .and_then(|states| states.get_mut(&self.blob_id)) else {
            unreachable!();
        };

        match state {
            State::Read(count) => {
                *state = State::Write(count.checked_add(1).expect("lock count limit exceeded"));

                Some(WriteLock {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id: self.blob_id,
                })
            }
            State::Write(_) => None,
            State::Unique(_) => unreachable!(),
        }
    }
}

impl Clone for ReadLock {
    fn clone(&self) -> Self {
        let mut shared = self.shared.lock().unwrap();

        let Some(state) = shared
            .get_mut(&self.branch_id)
            .and_then(|states| states.get_mut(&self.blob_id)) else {
            unreachable!();
        };

        match state {
            State::Read(count) | State::Write(count) => {
                *count = count.checked_add(1).expect("lock count limit exceeded");

                Self {
                    shared: self.shared.clone(),
                    branch_id: self.branch_id,
                    blob_id: self.blob_id,
                }
            }
            State::Unique(_) => unreachable!(),
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

        match state_entry.get_mut() {
            State::Read(count) | State::Write(count) => {
                *count = count.checked_sub(1).expect("lock count cannot be zero");

                if *count == 0 {
                    state_entry.remove();
                }
            }
            State::Unique(_) => unreachable!(),
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

        match state_entry.get_mut() {
            State::Write(count) => {
                *count = count.checked_sub(1).expect("lock count cannot be zero");

                if *count > 0 {
                    *state_entry.get_mut() = State::Read(*count);
                } else {
                    state_entry.remove();
                }
            }
            State::Read(_) | State::Unique(_) => unreachable!(),
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

impl UniqueLock {
    pub fn blob_id(&self) -> &BlobId {
        &self.blob_id
    }

    pub fn branch_id(&self) -> &PublicKey {
        &self.branch_id
    }
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

        match state_entry.get_mut() {
            State::Unique(notify) => {
                notify.notify_waiters();
                state_entry.remove();
            }
            State::Read(_) | State::Write(_) => unreachable!(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanity_check() {
        let branch_id = PublicKey::random();
        let blob_id: BlobId = rand::random();

        let locker = Locker::new();
        let locker = locker.branch(branch_id);

        let read0 = locker.read(blob_id).unwrap();
        let read1 = locker.read(blob_id).unwrap();
        let read2 = read0.clone();

        let write0 = read0.upgrade().unwrap();
        assert!(read0.upgrade().is_none());

        drop(write0);
        let write1 = read0.upgrade().unwrap();

        assert!(locker.unique(blob_id).is_none());

        drop(write1);
        assert!(locker.unique(blob_id).is_none());

        drop(read2);
        assert!(locker.unique(blob_id).is_none());

        drop(read1);
        assert!(locker.unique(blob_id).is_none());

        drop(read0);
        let remove0 = locker.unique(blob_id).unwrap();

        assert!(locker.read(blob_id).is_none());
        assert!(locker.unique(blob_id).is_none());

        drop(remove0);
        let remove1 = locker.unique(blob_id).unwrap();

        drop(remove1);
        let _read3 = locker.read(blob_id).unwrap();
    }
}

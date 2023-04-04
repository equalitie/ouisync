//! File write locks
//!
//! For simplicity, concurrent writes to the same file are currently not allowed but this should
//! change in the future. See also https://github.com/equalitie/ouisync/issues/96

use crate::{blob_id::BlobId, collections::HashSet, deadlock::BlockingMutex};
use std::sync::Arc;

/// Lock that permits write access to a file.
pub(crate) struct FileWriteLock {
    shared: Arc<BlockingMutex<HashSet<BlobId>>>,
    blob_id: BlobId,
}

impl Drop for FileWriteLock {
    fn drop(&mut self) {
        let mut shared = self.shared.lock().unwrap();

        if !shared.remove(&self.blob_id) {
            unreachable!()
        }
    }
}

#[derive(Clone)]
pub(crate) struct FileWriteLocker {
    shared: Arc<BlockingMutex<HashSet<BlobId>>>,
}

impl FileWriteLocker {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(BlockingMutex::new(HashSet::new())),
        }
    }

    /// Acquire write lock for a file with the given id. Returns `None` if already acquired.
    pub fn lock(&self, blob_id: BlobId) -> Option<FileWriteLock> {
        let mut shared = self.shared.lock().unwrap();

        if shared.insert(blob_id) {
            Some(FileWriteLock {
                shared: self.shared.clone(),
                blob_id,
            })
        } else {
            None
        }
    }
}

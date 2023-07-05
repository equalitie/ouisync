use crate::{
    blob::{lock::UpgradableLock, Blob},
    block::BLOCK_SIZE,
    branch::Branch,
    db,
    directory::{Directory, ParentContext},
    error::{Error, Result},
    index::VersionVectorOp,
    locator::Locator,
    version_vector::VersionVector,
};
use std::{fmt, future::Future, io::SeekFrom};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::instrument;

// When this many blocks are written, the shared transaction is forcefully commited. This prevents
// the shared transaction from becoming too big.
const MAX_UNCOMMITTED_BLOCKS: u32 = 64;

// Maximum number of `File.progress` calls that can be executed concurrently. We limit this because
// the operation is currently slow and we don't want to exhaust all db connections when multiple
// files are queries for progress.
pub(crate) const MAX_CONCURRENT_FILE_PROGRESS_QUERIES: usize = 3;

pub struct File {
    blob: Blob,
    parent: ParentContext,
    lock: UpgradableLock,
}

impl File {
    /// Opens an existing file.
    pub(crate) async fn open(
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
    ) -> Result<Self> {
        let lock = branch.locker().try_read(*locator.blob_id()).map_err(|_| {
            tracing::warn!(
                branch_id = ?branch.id(),
                blob_id = ?locator.blob_id(),
                "failed to acquire read lock"
            );

            Error::EntryNotFound
        })?;
        let lock = UpgradableLock::Read(lock);

        let mut tx = branch.db().begin_read().await?;

        Ok(Self {
            blob: Blob::open(&mut tx, branch, locator).await?,
            parent,
            lock,
        })
    }

    /// Creates a new file.
    pub(crate) fn create(branch: Branch, locator: Locator, parent: ParentContext) -> Self {
        // The only way this could fail is if there is already another locked blob with the same id
        // in the same branch. But the blob id is randomly generated so this would imply we
        // generated the same id more than once which is so astronomically unlikely that we might
        // as well panic.
        let lock = branch
            .locker()
            .try_read(*locator.blob_id())
            .ok()
            .expect("blob_id collision");
        let lock = UpgradableLock::Read(lock);

        Self {
            blob: Blob::create(branch, locator),
            parent,
            lock,
        }
    }

    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }

    pub async fn parent(&self) -> Result<Directory> {
        self.parent.open(self.branch().clone()).await
    }

    /// Length of this file in bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.blob.len()
    }

    /// Sync progress of this file, that is, what part of this file (in bytes) is available locally.
    /// NOTE: The future returned from this function doesn't borrow from `self` so it's possible
    /// to drop the `self` before/while awaiting it. This is useful to avoid keeping the file lock
    /// while awaiting the result.
    pub fn progress(&self) -> impl Future<Output = Result<u64>> {
        let branch = self.branch().clone();
        let locator = *self.blob.locator();
        let block_count = self.blob.block_count();

        async move {
            let _permit = branch.acquire_file_progress_query_permit().await;

            let mut tx = branch.db().begin_read().await?;
            let snapshot = branch.data().load_snapshot(&mut tx).await?;

            let mut count = 0u64;

            for locator in locator
                .sequence()
                .map(|locator| locator.encode(branch.keys().read()))
                .take(block_count as usize)
            {
                let (_, presence) = snapshot.get_block(&mut tx, &locator).await?;

                if presence.is_present() {
                    count = count.saturating_add(1);
                }
            }

            Ok(count * BLOCK_SIZE as u64)
        }
    }

    /// Reads data from this file. See [`Blob::read`] for more info.
    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let mut tx = self.branch().db().begin_read().await?;
        self.blob.read(&mut tx, buffer).await
    }

    /// Read all data from this file from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        let mut tx = self.branch().db().begin_read().await?;
        self.blob.read_to_end(&mut tx).await
    }

    /// Writes `buffer` into this file.
    #[instrument(skip_all, fields(buffer.len = buffer.len()))]
    pub async fn write(&mut self, buffer: &[u8]) -> Result<()> {
        let mut tx = self.branch().db().begin_shared_write().await?;
        self.acquire_write_lock()?;
        let block_written = self.blob.write(&mut tx, buffer).await?;

        if block_written
            && self.branch().uncommitted_block_counter().increment() == MAX_UNCOMMITTED_BLOCKS
        {
            tx.commit().await?;
            self.branch().uncommitted_block_counter().reset();
        }

        Ok(())
    }

    /// Seeks to an offset in the file.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let mut tx = self.branch().db().begin_read().await?;
        self.blob.seek(&mut tx, pos).await
    }

    /// Truncates the file to the given length.
    #[instrument(skip(self))]
    pub async fn truncate(&mut self, len: u64) -> Result<()> {
        let mut tx = self.branch().db().begin_read().await?;
        self.acquire_write_lock()?;
        self.blob.truncate(&mut tx, len).await
    }

    /// Atomically saves any pending modifications and updates the version vectors of this file and
    /// all its ancestors.
    #[instrument(skip_all)]
    pub async fn flush(&mut self) -> Result<()> {
        if !self.blob.is_dirty() {
            return Ok(());
        }

        let mut tx = self.branch().db().begin_shared_write().await?;
        self.blob.flush(&mut tx).await?;
        self.parent
            .bump(
                &mut tx,
                self.branch().clone(),
                VersionVectorOp::IncrementLocal,
            )
            .await?;

        let uncommitted_block_counter = self.branch().uncommitted_block_counter().clone();
        let event_tx = self.branch().notify();

        tx.commit_and_then(move || {
            uncommitted_block_counter.reset();
            event_tx.send();
        })
        .await?;

        Ok(())
    }

    /// Saves any pending modifications but does not update the version vectors. For internal use
    /// only.
    pub(crate) async fn save(&mut self, tx: &mut db::WriteTransaction) -> Result<()> {
        self.blob.flush(tx).await?;
        Ok(())
    }

    /// Copy the entire contents of this file into the provided writer (e.g. a file on a regular
    /// filesystem)
    pub async fn copy_to_writer<W: AsyncWrite + Unpin>(&mut self, dst: &mut W) -> Result<()> {
        let mut buffer = vec![0; BLOCK_SIZE];

        loop {
            let len = self.read(&mut buffer).await?;
            dst.write_all(&buffer[..len]).await.map_err(Error::Writer)?;

            if len < buffer.len() {
                break;
            }
        }

        Ok(())
    }

    /// Forks this file into the given branch. Ensure all its ancestor directories exist and live
    /// in the branch as well. Should be called before any mutable operation.
    pub async fn fork(&mut self, dst_branch: Branch) -> Result<()> {
        if self.branch().id() == dst_branch.id() {
            // File already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let parent = self.parent.fork(self.branch(), &dst_branch).await?;

        let lock = dst_branch
            .locker()
            .read(*self.blob.locator().blob_id())
            .await;
        let lock = UpgradableLock::Read(lock);

        let blob = {
            let mut tx = dst_branch.db().begin_read().await?;
            Blob::open(&mut tx, dst_branch, *self.blob.locator()).await?
        };

        *self = Self { blob, parent, lock };

        Ok(())
    }

    pub async fn version_vector(&self) -> Result<VersionVector> {
        self.parent
            .entry_version_vector(self.branch().clone())
            .await
    }

    /// Locator of this file.
    #[cfg(test)]
    pub(crate) fn locator(&self) -> &Locator {
        self.blob.locator()
    }

    fn acquire_write_lock(&mut self) -> Result<()> {
        self.lock.upgrade().then_some(()).ok_or(Error::Locked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access_control::{AccessKeys, WriteSecrets},
        branch::BranchShared,
        crypto::sign::PublicKey,
        db,
        directory::{DirectoryFallback, DirectoryLocking},
        event::EventSender,
        index::BranchData,
        test_utils,
    };
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        test_utils::init_log();
        let (_base_dir, [branch0, branch1]) = setup().await;

        // Create a file owned by branch 0
        let mut file0 = branch0.ensure_file_exists("dog.jpg".into()).await.unwrap();
        file0.write(b"small").await.unwrap();
        file0.flush().await.unwrap();
        drop(file0);

        // Open the file, fork it into branch 1 and modify it.
        let mut file1 = branch0
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        file1.fork(branch1.clone()).await.unwrap();
        file1.write(b"large").await.unwrap();
        file1.flush().await.unwrap();

        // Reopen orig file and verify it's unchanged
        let mut file = branch0
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        assert_eq!(file.read_to_end().await.unwrap(), b"small");

        // Reopen forked file and verify it's modified
        let mut file = branch1
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        assert_eq!(file.read_to_end().await.unwrap(), b"large");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_consecutive_modifications_of_forked_file() {
        // This test makes sure that modifying a forked file properly updates the file metadata so
        // subsequent modifications work correclty.

        let (_base_dir, [branch0, branch1]) = setup().await;

        let mut file0 = branch0.ensure_file_exists("/pig.jpg".into()).await.unwrap();
        file0.flush().await.unwrap();

        let mut file1 = branch0
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("pig.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        file1.fork(branch1).await.unwrap();

        for _ in 0..2 {
            file1.write(b"oink").await.unwrap();
            file1.flush().await.unwrap();
        }
    }

    // TODO: currently concurrent writes are not allowed and this tests simply asserts that. When
    // concurrent writes are implemented, we should remove this test and replace it with a series
    // of tests for various write concurrency cases.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_writes() {
        let (_base_dir, [branch]) = setup().await;

        let mut file0 = branch.ensure_file_exists("fox.txt".into()).await.unwrap();
        let mut file1 = branch
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("fox.txt")
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        file0.write(b"yip-yap").await.unwrap();
        assert_matches!(file1.write(b"ring-ding-ding").await, Err(Error::Locked));
        assert_matches!(file1.truncate(0).await, Err(Error::Locked));
    }

    async fn setup<const N: usize>() -> (TempDir, [Branch; N]) {
        let (base_dir, pool) = db::create_temp().await.unwrap();
        let keys = AccessKeys::from(WriteSecrets::random());
        let event_tx = EventSender::new(1);
        let shared = BranchShared::new();

        let branches = [(); N]
            .map(|_| create_branch(pool.clone(), event_tx.clone(), keys.clone(), shared.clone()));

        (base_dir, branches)
    }

    fn create_branch(
        pool: db::Pool,
        event_tx: EventSender,
        keys: AccessKeys,
        shared: BranchShared,
    ) -> Branch {
        let id = PublicKey::random();
        let branch_data = BranchData::new(id);
        Branch::new(pool, branch_data, keys, shared, event_tx)
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("File")
            .field("blob_id", &self.blob.locator().blob_id())
            .field("branch", &self.blob.branch().id())
            .finish()
    }
}

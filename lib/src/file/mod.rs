mod lock;

pub(crate) use self::lock::{FileWriteLock, FileWriteLocker};

use crate::{
    blob::Blob,
    block::BLOCK_SIZE,
    branch::Branch,
    db,
    directory::{Directory, ParentContext},
    error::{Error, Result},
    index::VersionVectorOp,
    locator::Locator,
    version_vector::VersionVector,
};
use std::{fmt, io::SeekFrom};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::instrument;

// When this many blocks are written, the shared transaction is forcefully commited. This prevents
// the shared transaction from becoming too big.
const MAX_UNCOMMITTED_BLOCKS: u32 = 64;

pub struct File {
    blob: Blob,
    parent: ParentContext,
    write_lock: Option<FileWriteLock>,
}

impl File {
    /// Opens an existing file.
    pub(crate) async fn open(
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
    ) -> Result<Self> {
        let mut tx = branch.db().begin_read().await?;

        Ok(Self {
            blob: Blob::open(&mut tx, branch, locator).await?,
            parent,
            write_lock: None,
        })
    }

    /// Creates a new file.
    pub(crate) fn create(branch: Branch, locator: Locator, parent: ParentContext) -> Self {
        Self {
            blob: Blob::create(branch, locator),
            parent,
            write_lock: None,
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
                &VersionVectorOp::IncrementLocal,
            )
            .await?;
        tx.commit().await?;

        self.branch().uncommitted_block_counter().reset();
        self.branch().data().notify();

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

        let new_parent = self.parent.fork(self.branch(), &dst_branch).await?;

        let new_blob = {
            let mut tx = dst_branch.db().begin_read().await?;
            Blob::open(&mut tx, dst_branch.clone(), *self.blob.locator()).await?
        };

        self.blob = new_blob;
        self.parent = new_parent;
        self.write_lock = None;

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
        if self.write_lock.is_none() {
            self.write_lock = Some(
                self.branch()
                    .lock_file_for_write(*self.blob.locator().blob_id())
                    .ok_or(Error::ConcurrentWriteNotSupported)?,
            );
        }

        Ok(())
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
        directory::MissingBlockStrategy,
        event::Event,
        index::BranchData,
    };
    use assert_matches::assert_matches;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let (_base_dir, [branch0, branch1]) = setup().await;

        // Create a file owned by branch 0
        let mut file0 = branch0.ensure_file_exists("dog.jpg".into()).await.unwrap();
        file0.write(b"small").await.unwrap();
        file0.flush().await.unwrap();
        drop(file0);

        // Open the file, fork it into branch 1 and modify it.
        let mut file1 = branch0
            .open_root(MissingBlockStrategy::Fail)
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
            .open_root(MissingBlockStrategy::Fail)
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
            .open_root(MissingBlockStrategy::Fail)
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
            .open_root(MissingBlockStrategy::Fail)
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
            .open_root(MissingBlockStrategy::Fail)
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
        assert_matches!(
            file1.write(b"ring-ding-ding").await,
            Err(Error::ConcurrentWriteNotSupported)
        );
        assert_matches!(
            file1.truncate(0).await,
            Err(Error::ConcurrentWriteNotSupported)
        );
    }

    async fn setup<const N: usize>() -> (TempDir, [Branch; N]) {
        let (base_dir, pool) = db::create_temp().await.unwrap();
        let keys = AccessKeys::from(WriteSecrets::random());
        let (event_tx, _) = broadcast::channel(1);
        let shared = BranchShared::new(event_tx.clone());

        let branches = [(); N]
            .map(|_| create_branch(pool.clone(), event_tx.clone(), keys.clone(), shared.clone()));

        (base_dir, branches)
    }

    fn create_branch(
        pool: db::Pool,
        event_tx: broadcast::Sender<Event>,
        keys: AccessKeys,
        shared: BranchShared,
    ) -> Branch {
        let branch_data = BranchData::new(PublicKey::random(), event_tx);
        Branch::new(pool, branch_data, keys, shared)
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

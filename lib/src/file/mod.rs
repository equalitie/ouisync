mod cache;
mod lock;

use self::lock::WriteLock;
pub(crate) use self::{cache::FileCache, lock::OpenLock};
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

pub struct File {
    blob: Blob,
    parent: ParentContext,
    write_lock: WriteLock,
}

impl File {
    /// Opens an existing file.
    pub(crate) async fn open(
        conn: &mut db::Connection,
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
    ) -> Result<Self> {
        let write_lock = WriteLock::new(branch.acquire_open_lock(*locator.blob_id()));

        Ok(Self {
            blob: Blob::open(conn, branch, locator).await?,
            parent,
            write_lock,
        })
    }

    /// Creates a new file.
    pub(crate) fn create(branch: Branch, locator: Locator, parent: ParentContext) -> Self {
        let write_lock = WriteLock::new(branch.acquire_open_lock(*locator.blob_id()));

        Self {
            blob: Blob::create(branch, locator),
            parent,
            write_lock,
        }
    }

    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }

    pub async fn parent(&self, conn: &mut db::Connection) -> Result<Directory> {
        self.parent.directory(conn, self.branch().clone()).await
    }

    /// Length of this file in bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.blob.len()
    }

    /// Reads data from this file. See [`Blob::read`] for more info.
    pub async fn read(&mut self, conn: &mut db::Connection, buffer: &mut [u8]) -> Result<usize> {
        self.blob.read(conn, buffer).await
    }

    /// Read all data from this file from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self, conn: &mut db::Connection) -> Result<Vec<u8>> {
        self.blob.read_to_end(conn).await
    }

    /// Writes `buffer` into this file.
    pub async fn write(&mut self, conn: &mut db::PoolConnection, buffer: &[u8]) -> Result<()> {
        let mut tx = conn.begin().await?;
        self.acquire_write_lock()?;
        self.blob.write(&mut tx, buffer).await?;
        tx.commit().await?;

        Ok(())
    }

    /// Seeks to an offset in the file.
    pub async fn seek(&mut self, conn: &mut db::Connection, pos: SeekFrom) -> Result<u64> {
        self.blob.seek(conn, pos).await
    }

    /// Truncates the file to the given length.
    pub async fn truncate(&mut self, conn: &mut db::Connection, len: u64) -> Result<()> {
        self.acquire_write_lock()?;
        self.blob.truncate(conn, len).await?;

        Ok(())
    }

    /// Atomically saves any pending modifications and updates the version vectors of this file and
    /// all its ancestors.
    pub async fn flush(&mut self, conn: &mut db::PoolConnection) -> Result<()> {
        if !self.blob.is_dirty() {
            return Ok(());
        }

        let mut tx = conn.begin().await?;
        self.blob.flush(&mut tx).await?;
        self.parent
            .bump(
                &mut tx,
                self.branch().clone(),
                &VersionVectorOp::IncrementLocal,
            )
            .await?;
        tx.commit().await?;
        self.branch().data().notify();

        Ok(())
    }

    /// Saves any pending modifications but does not update the version vectors. For internal use
    /// only.
    pub(crate) async fn save(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        self.blob.flush(tx).await?;
        Ok(())
    }

    /// Copy the entire contents of this file into the provided writer (e.g. a file on a regular
    /// filesystem)
    pub async fn copy_to_writer<W: AsyncWrite + Unpin>(
        &mut self,
        conn: &mut db::Connection,
        dst: &mut W,
    ) -> Result<()> {
        let mut buffer = vec![0; BLOCK_SIZE];

        loop {
            let len = self.read(conn, &mut buffer).await?;
            dst.write_all(&buffer[..len]).await.map_err(Error::Writer)?;

            if len < buffer.len() {
                break;
            }
        }

        Ok(())
    }

    /// Forks this file into the given branch. Ensure all its ancestor directories exist and live
    /// in the branch as well. Should be called before any mutable operation.
    pub async fn fork(&mut self, conn: &mut db::PoolConnection, dst_branch: Branch) -> Result<()> {
        if self.branch().id() == dst_branch.id() {
            // File already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let (new_parent, new_blob) = self
            .parent
            .fork(conn, &self.blob, self.branch().clone(), dst_branch.clone())
            .await?;

        self.blob = new_blob;
        self.parent = new_parent;
        self.write_lock =
            WriteLock::new(dst_branch.acquire_open_lock(*self.blob.locator().blob_id()));

        Ok(())
    }

    pub async fn version_vector(&self, conn: &mut db::Connection) -> Result<VersionVector> {
        self.parent
            .entry_version_vector(conn, self.branch().clone())
            .await
    }

    /// Locator of this file.
    #[cfg(test)]
    pub(crate) fn locator(&self) -> &Locator {
        self.blob.locator()
    }

    fn acquire_write_lock(&mut self) -> Result<()> {
        self.write_lock
            .acquire()
            .then_some(())
            .ok_or(Error::ConcurrentWriteNotSupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access_control::{AccessKeys, WriteSecrets},
        crypto::sign::PublicKey,
        db,
        event::Event,
        index::BranchData,
    };
    use assert_matches::assert_matches;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let (_base_dir, pool, [branch0, branch1]) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        // Create a file owned by branch 0
        let mut file0 = branch0
            .ensure_file_exists(&mut conn, "/dog.jpg".into())
            .await
            .unwrap();

        file0.write(&mut conn, b"small").await.unwrap();
        file0.flush(&mut conn).await.unwrap();

        // Open the file, fork it into branch 1 and modify it.
        let mut file1 = branch0
            .open_root(&mut conn)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut conn)
            .await
            .unwrap();

        file1.fork(&mut conn, branch1.clone()).await.unwrap();
        file1.write(&mut conn, b"large").await.unwrap();
        file1.flush(&mut conn).await.unwrap();

        // Reopen orig file and verify it's unchanged
        let mut file = branch0
            .open_root(&mut conn)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut conn)
            .await
            .unwrap();

        assert_eq!(file.read_to_end(&mut conn).await.unwrap(), b"small");

        // Reopen forked file and verify it's modified
        let mut file = branch1
            .open_root(&mut conn)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut conn)
            .await
            .unwrap();

        assert_eq!(file.read_to_end(&mut conn).await.unwrap(), b"large");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_consecutive_modifications_of_forked_file() {
        // This test makes sure that modifying a forked file properly updates the file metadata so
        // subsequent modifications work correclty.

        let (_base_dir, pool, [branch0, branch1]) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let mut file0 = branch0
            .ensure_file_exists(&mut conn, "/pig.jpg".into())
            .await
            .unwrap();
        file0.flush(&mut conn).await.unwrap();

        let mut file1 = branch0
            .open_root(&mut conn)
            .await
            .unwrap()
            .lookup("pig.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut conn)
            .await
            .unwrap();

        file1.fork(&mut conn, branch1).await.unwrap();

        for _ in 0..2 {
            file1.write(&mut conn, b"oink").await.unwrap();
            file1.flush(&mut conn).await.unwrap();
        }
    }

    // TODO: currently concurrent writes are not allowed and this tests simply asserts that. When
    // concurrent writes are implemented, we should remove this test and replace it with a series
    // of tests for various write concurrency cases.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_writes() {
        let (_base_dir, pool, [branch]) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let mut file0 = branch
            .ensure_file_exists(&mut conn, "fox.txt".into())
            .await
            .unwrap();
        let mut file1 = branch
            .open_root(&mut conn)
            .await
            .unwrap()
            .lookup("fox.txt")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut conn)
            .await
            .unwrap();

        file0.write(&mut conn, b"yip-yap").await.unwrap();
        assert_matches!(
            file1.write(&mut conn, b"ring-ding-ding").await,
            Err(Error::ConcurrentWriteNotSupported)
        );
    }

    async fn setup<const N: usize>() -> (TempDir, db::Pool, [Branch; N]) {
        let (base_dir, pool) = db::create_temp().await.unwrap();
        let keys = AccessKeys::from(WriteSecrets::random());
        let (event_tx, _) = broadcast::channel(1);
        let file_cache = Arc::new(FileCache::new(event_tx.clone()));

        let branches =
            [(); N].map(|_| create_branch(event_tx.clone(), keys.clone(), file_cache.clone()));

        (base_dir, pool, branches)
    }

    fn create_branch(
        event_tx: broadcast::Sender<Event>,
        keys: AccessKeys,
        file_cache: Arc<FileCache>,
    ) -> Branch {
        let branch_data = BranchData::new(PublicKey::random(), event_tx);
        Branch::new(Arc::new(branch_data), keys, file_cache)
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

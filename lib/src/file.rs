use crate::{
    blob::{Blob, MaybeInitShared},
    block::BLOCK_SIZE,
    branch::Branch,
    db,
    directory::{Directory, ParentContext},
    error::{Error, Result},
    locator::Locator,
    version_vector::VersionVector,
};
use sqlx::Connection;
use std::fmt;
use std::io::SeekFrom;
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub struct File {
    blob: Blob,
    parent: ParentContext,
}

impl File {
    /// Opens an existing file.
    pub(crate) async fn open(
        conn: &mut db::Connection,
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
        blob_shared: MaybeInitShared,
    ) -> Result<Self> {
        Ok(Self {
            blob: Blob::open(conn, branch, locator, blob_shared).await?,
            parent,
        })
    }

    /// Creates a new file.
    pub(crate) fn create(
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
        blob_shared: MaybeInitShared,
    ) -> Self {
        Self {
            blob: Blob::create(branch, locator, blob_shared),
            parent,
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
    pub async fn len(&self) -> u64 {
        self.blob.len().await
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
    pub async fn write(&mut self, conn: &mut db::Connection, buffer: &[u8]) -> Result<()> {
        self.blob.write(conn, buffer).await
    }

    /// Seeks to an offset in the file.
    pub async fn seek(&mut self, conn: &mut db::Connection, pos: SeekFrom) -> Result<u64> {
        self.blob.seek(conn, pos).await
    }

    /// Truncates the file to the given length.
    pub async fn truncate(&mut self, conn: &mut db::Connection, len: u64) -> Result<()> {
        self.blob.truncate(conn, len).await
    }

    /// Atomically saves any pending modifications and updates the version vectors of this file and
    /// all its ancestors.
    pub async fn flush(&mut self, conn: &mut db::Connection) -> Result<()> {
        if !self.blob.is_dirty() {
            return Ok(());
        }

        let mut tx = conn.begin().await?;
        self.blob.flush(&mut tx).await?;
        self.parent
            .commit(tx, self.branch().clone(), VersionVector::new())
            .await
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
    pub async fn fork(&mut self, conn: &mut db::Connection, dst_branch: Branch) -> Result<()> {
        if self.branch().id() == dst_branch.id() {
            // File already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let tx = conn.begin().await?;
        let (new_parent, new_blob) = self
            .parent
            .fork(tx, &self.blob, self.branch().clone(), dst_branch)
            .await?;

        self.blob = new_blob;
        self.parent = new_parent;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        access_control::{AccessKeys, WriteSecrets},
        blob::BlobCache,
        crypto::sign::PublicKey,
        db,
        index::BranchData,
        sync::broadcast,
    };
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let (pool, branch0, branch1) = setup().await;
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
            .read()
            .await
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
            .read()
            .await
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
            .read()
            .await
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

        let (pool, branch0, branch1) = setup().await;
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
            .read()
            .await
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

    async fn setup() -> (db::Pool, Branch, Branch) {
        let pool = db::create(&db::Store::Temporary).await.unwrap();
        let keys = AccessKeys::from(WriteSecrets::random());
        let blob_cache = Arc::new(BlobCache::new());

        let branch0 = create_branch(&pool, keys.clone(), blob_cache.clone()).await;
        let branch1 = create_branch(&pool, keys.clone(), blob_cache).await;

        (pool, branch0, branch1)
    }

    async fn create_branch(
        pool: &db::Pool,
        keys: AccessKeys,
        blob_cache: Arc<BlobCache>,
    ) -> Branch {
        let notify_tx = broadcast::Sender::new(1);
        let branch_data = BranchData::create(
            &mut pool.acquire().await.unwrap(),
            PublicKey::random(),
            keys.write().unwrap(),
            notify_tx,
        )
        .await
        .unwrap();
        Branch::new(Arc::new(branch_data), keys, blob_cache)
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

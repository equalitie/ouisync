use crate::{
    blob::{Blob, MaybeInitShared},
    block::BLOCK_SIZE,
    branch::Branch,
    db,
    directory::{Directory, ParentContext},
    error::{Error, Result},
    index::VersionVectorOp,
    locator::Locator,
    version_vector::VersionVector,
};
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
    pub(crate) async fn save(&mut self, conn: &mut db::Connection) -> Result<()> {
        self.blob.flush(conn).await?;
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

        let (new_parent, new_blob) = self
            .parent
            .fork(conn, &self.blob, self.branch().clone(), dst_branch)
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
        event::Event,
        index::BranchData,
    };
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let (_base_dir, pool, branch0, branch1) = setup().await;
        let mut tx = pool.begin().await.unwrap();

        // Create a file owned by branch 0
        let mut file0 = branch0
            .ensure_file_exists(&mut tx, "/dog.jpg".into())
            .await
            .unwrap();

        file0.write(&mut tx, b"small").await.unwrap();
        file0.flush(&mut tx).await.unwrap();

        // Open the file, fork it into branch 1 and modify it.
        let mut file1 = branch0
            .open_root(&mut tx)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut tx)
            .await
            .unwrap();

        file1.fork(&mut tx, branch1.clone()).await.unwrap();
        file1.write(&mut tx, b"large").await.unwrap();
        file1.flush(&mut tx).await.unwrap();

        // Reopen orig file and verify it's unchanged
        let mut file = branch0
            .open_root(&mut tx)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut tx)
            .await
            .unwrap();

        assert_eq!(file.read_to_end(&mut tx).await.unwrap(), b"small");

        // Reopen forked file and verify it's modified
        let mut file = branch1
            .open_root(&mut tx)
            .await
            .unwrap()
            .lookup("dog.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut tx)
            .await
            .unwrap();

        assert_eq!(file.read_to_end(&mut tx).await.unwrap(), b"large");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_consecutive_modifications_of_forked_file() {
        // This test makes sure that modifying a forked file properly updates the file metadata so
        // subsequent modifications work correclty.

        let (_base_dir, pool, branch0, branch1) = setup().await;
        let mut tx = pool.begin().await.unwrap();

        let mut file0 = branch0
            .ensure_file_exists(&mut tx, "/pig.jpg".into())
            .await
            .unwrap();
        file0.flush(&mut tx).await.unwrap();

        let mut file1 = branch0
            .open_root(&mut tx)
            .await
            .unwrap()
            .lookup("pig.jpg")
            .unwrap()
            .file()
            .unwrap()
            .open(&mut tx)
            .await
            .unwrap();

        file1.fork(&mut tx, branch1).await.unwrap();

        for _ in 0..2 {
            file1.write(&mut tx, b"oink").await.unwrap();
            file1.flush(&mut tx).await.unwrap();
        }
    }

    async fn setup() -> (TempDir, db::Pool, Branch, Branch) {
        let (base_dir, pool) = db::create_temp().await.unwrap();
        let keys = AccessKeys::from(WriteSecrets::random());
        let (event_tx, _) = broadcast::channel(1);
        let blob_cache = Arc::new(BlobCache::new(event_tx.clone()));

        let branch0 = create_branch(event_tx.clone(), keys.clone(), blob_cache.clone());
        let branch1 = create_branch(event_tx, keys, blob_cache);

        (base_dir, pool, branch0, branch1)
    }

    fn create_branch(
        event_tx: broadcast::Sender<Event>,
        keys: AccessKeys,
        blob_cache: Arc<BlobCache>,
    ) -> Branch {
        let branch_data = BranchData::new(PublicKey::random(), event_tx);
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

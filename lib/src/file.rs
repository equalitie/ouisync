use crate::{
    blob::{Blob, MaybeInitShared, UninitShared},
    block::BLOCK_SIZE,
    branch::Branch,
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
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
        blob_shared: MaybeInitShared,
    ) -> Result<Self> {
        Ok(Self {
            blob: Blob::open(branch, locator, blob_shared).await?,
            parent,
        })
    }

    /// Creates a new file.
    pub(crate) fn create(
        branch: Branch,
        locator: Locator,
        parent: ParentContext,
        blob_shared: UninitShared,
    ) -> Self {
        Self {
            blob: Blob::create(branch, locator, blob_shared),
            parent,
        }
    }

    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }

    pub fn parent(&self) -> Directory {
        self.parent.directory().clone()
    }

    /// Length of this file in bytes.
    #[allow(clippy::len_without_is_empty)]
    pub async fn len(&self) -> u64 {
        self.blob.len().await
    }

    /// Reads data from this file. See [`Blob::read`] for more info.
    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.blob.read(buffer).await
    }

    /// Read all data from this file from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        self.blob.read_to_end().await
    }

    /// Writes `buffer` into this file.
    pub async fn write(&mut self, buffer: &[u8]) -> Result<()> {
        self.blob.write(buffer).await
    }

    /// Seeks to an offset in the file.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.blob.seek(pos).await
    }

    /// Truncates the file to the given length.
    pub async fn truncate(&mut self, len: u64) -> Result<()> {
        self.blob.truncate(len).await
    }

    /// Flushes this file, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self) -> Result<()> {
        if !self.blob.is_dirty() {
            return Ok(());
        }

        let increment = VersionVector::first(*self.branch().id());

        let mut conn = self.blob.db_pool().acquire().await?;
        let mut tx = conn.begin().await?;

        self.blob.flush_in_transaction(&mut tx).await?;
        self.parent.modify_entry(tx, &increment).await?;

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

    /// Forks this file into the local branch. Ensure all its ancestor directories exist and live
    /// in the local branch as well. Should be called before any mutable operation.
    pub async fn fork(&mut self, local_branch: &Branch) -> Result<()> {
        if self.blob.branch().id() == local_branch.id() {
            // File already lives in the local branch. We assume the ancestor directories have been
            // already created as well so there is nothing else to do.
            return Ok(());
        }

        let blob_id = rand::random();
        self.blob
            .fork(local_branch.clone(), Locator::head(blob_id))
            .await?;
        self.parent.fork_file(local_branch, blob_id).await?;

        Ok(())
    }

    pub async fn version_vector(&self) -> VersionVector {
        self.parent.entry_version_vector().await
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
        crypto::sign::PublicKey,
        db,
        index::BranchData,
        sync::broadcast,
    };
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let (branch0, branch1) = setup().await;

        // Create a file owned by branch 0
        let mut file0 = branch0.ensure_file_exists("/dog.jpg".into()).await.unwrap();

        file0.write(b"small").await.unwrap();
        file0.flush().await.unwrap();

        // Open the file, fork it into branch 1 and modify it.
        let mut file1 = branch0
            .open_root()
            .await
            .unwrap()
            .read()
            .await
            .lookup_version("dog.jpg", branch0.id())
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        file1.fork(&branch1).await.unwrap();
        file1.write(b"large").await.unwrap();
        file1.flush().await.unwrap();

        // Reopen orig file and verify it's unchanged
        let mut file = branch0
            .open_root()
            .await
            .unwrap()
            .read()
            .await
            .lookup_version("dog.jpg", branch0.id())
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        assert_eq!(file.read_to_end().await.unwrap(), b"small");

        // Reopen forked file and verify it's modified
        let mut file = branch1
            .open_root()
            .await
            .unwrap()
            .read()
            .await
            .lookup_version("dog.jpg", branch1.id())
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

        let (branch0, branch1) = setup().await;

        let mut file0 = branch0.ensure_file_exists("/pig.jpg".into()).await.unwrap();
        file0.flush().await.unwrap();

        let mut file1 = branch0
            .open_root()
            .await
            .unwrap()
            .read()
            .await
            .lookup_version("pig.jpg", branch0.id())
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();

        file1.fork(&branch1).await.unwrap();

        for _ in 0..2 {
            file1.write(b"oink").await.unwrap();
            file1.flush().await.unwrap();
        }
    }

    async fn setup() -> (Branch, Branch) {
        let pool = db::create(&db::Store::Temporary).await.unwrap();
        let keys = AccessKeys::from(WriteSecrets::random());

        (
            create_branch(pool.clone(), keys.clone()).await,
            create_branch(pool, keys).await,
        )
    }

    async fn create_branch(pool: db::Pool, keys: AccessKeys) -> Branch {
        let notify_tx = broadcast::Sender::new(1);
        let branch_data = BranchData::create(
            &mut pool.acquire().await.unwrap(),
            PublicKey::random(),
            keys.write().unwrap(),
            notify_tx,
        )
        .await
        .unwrap();
        Branch::new(pool, Arc::new(branch_data), keys)
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

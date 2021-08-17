use crate::{
    blob::Blob, blob_id::BlobId, branch::Branch, entry_type::EntryType, error::Result,
    locator::Locator, write_context::WriteContext,
};
use std::io::SeekFrom;
use std::sync::Arc;

pub struct File {
    blob: Blob,
    write_context: Arc<WriteContext>,
}

impl File {
    /// Opens an existing file.
    pub async fn open(
        branch: Branch,
        locator: Locator,
        write_context: Arc<WriteContext>,
    ) -> Result<Self> {
        Ok(Self {
            blob: Blob::open(branch, locator).await?,
            write_context,
        })
    }

    /// Creates a new file.
    pub fn create(branch: Branch, locator: Locator, write_context: Arc<WriteContext>) -> Self {
        Self {
            blob: Blob::create(branch, locator),
            write_context,
        }
    }

    pub fn branch(&self) -> &Branch {
        self.blob.branch()
    }

    /// Length of this file in bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.blob.len()
    }

    /// Locator of this file.
    pub fn locator(&self) -> &Locator {
        self.blob.locator()
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
        self.write_context
            .begin(EntryType::File, &mut self.blob)
            .await?;
        self.blob.write(buffer).await
    }

    /// Seeks to an offset in the file.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.blob.seek(pos).await
    }

    /// Truncates the file to the given length.
    pub async fn truncate(&mut self, len: u64) -> Result<()> {
        self.write_context
            .begin(EntryType::File, &mut self.blob)
            .await?;
        self.blob.truncate(len).await
    }

    /// Flushes this file, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self) -> Result<()> {
        self.blob.flush().await?;
        self.write_context.commit().await
    }

    /// Removes this file.
    pub async fn remove(self) -> Result<()> {
        // TODO: consider only allowing this if file is in the local branch.
        self.blob.remove().await
    }

    pub fn blob_id(&self) -> &BlobId {
        self.blob.blob_id()
    }
}

#[cfg(test)]
mod test {
    use crate::{crypto::Cryptor, db, index::BranchData};

    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn fork() {
        let branch0 = setup().await;
        let branch1 = create_branch(branch0.db_pool().clone()).await;

        // Create a file owned by branch 0
        let (mut file0, dirs) = branch0.ensure_file_exists("/dog.jpg".into()).await.unwrap();

        file0.write(b"small").await.unwrap();
        file0.flush().await.unwrap();
        for dir in dirs {
            dir.flush().await.unwrap();
        }

        // Write to the file by branch 1
        let mut file1 = branch0
            .open_root(branch1.clone())
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

        file1.write(b"large").await.unwrap();
        file1.flush().await.unwrap();

        // Reopen orig file and verify it's unchanged
        let mut file = branch0
            .open_root(branch0.clone())
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
            .open_root(branch1.clone())
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

    async fn setup() -> Branch {
        let pool = db::init(db::Store::Memory).await.unwrap();
        create_branch(pool).await
    }

    async fn create_branch(pool: db::Pool) -> Branch {
        let branch_data = BranchData::new(&pool, rand::random()).await.unwrap();
        Branch::new(pool, branch_data, Cryptor::Null)
    }
}

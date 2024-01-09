mod progress_cache;

pub(crate) use progress_cache::FileProgressCache;

use crate::{
    blob::{lock::UpgradableLock, Blob, ReadWriteError},
    branch::Branch,
    directory::{Directory, ParentContext},
    error::{Error, Result},
    protocol::{Bump, Locator, BLOCK_SIZE},
    store::{Changeset, ReadTransaction},
    version_vector::VersionVector,
};
use std::{fmt, future::Future, io::SeekFrom};
use tokio::io::{AsyncWrite, AsyncWriteExt};

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
        let lock = branch.locker().read(*locator.blob_id()).await;
        let lock = UpgradableLock::Read(lock);

        let mut tx = branch.store().begin_read().await?;

        Ok(Self {
            blob: Blob::open(&mut tx, branch, *locator.blob_id()).await?,
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
            blob: Blob::create(branch, *locator.blob_id()),
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
        let locator = Locator::head(*self.blob.id());
        let block_count = self.blob.block_count();
        let len = self.len();

        async move {
            let permit = branch.file_progress_cache().acquire().await;

            let mut tx = branch.store().begin_read().await?;
            let mut entry = permit.get(*locator.blob_id());
            let mut count = *entry;

            for index in *entry..block_count {
                let encoded_locator = locator.nth(index).encode(branch.keys().read());
                let block_id = tx.find_block(branch.id(), &encoded_locator).await?;

                if tx.block_exists(&block_id).await? {
                    count = count.saturating_add(1);
                } else {
                    break;
                }
            }

            *entry = count;

            Ok((count as u64 * BLOCK_SIZE as u64).min(len))
        }
    }

    /// Reads data from this file. Returns the number of bytes actually read.
    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        loop {
            match self.blob.read(buffer) {
                Ok(len) => return Ok(len),
                Err(ReadWriteError::CacheMiss) => {
                    let mut tx = self.branch().store().begin_read().await?;
                    self.blob.warmup(&mut tx).await?;
                }
                Err(ReadWriteError::CacheFull) => {
                    self.flush().await?;
                }
            }
        }
    }

    pub async fn read_all(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let mut offset = 0;

        loop {
            match self.read(&mut buffer[offset..]).await? {
                0 => return Ok(offset),
                n => {
                    offset += n;
                }
            }
        }
    }

    /// Read all data from this file from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        let mut buffer = vec![
            0;
            (self.blob.len() - self.blob.seek_position())
                .try_into()
                .unwrap_or(usize::MAX)
        ];
        self.read_all(&mut buffer[..]).await?;
        Ok(buffer)
    }

    /// Writes `buffer` into this file. Returns the number of bytes actually written.
    pub async fn write(&mut self, buffer: &[u8]) -> Result<usize> {
        self.acquire_write_lock()?;

        loop {
            match self.blob.write(buffer) {
                Ok(len) => return Ok(len),
                Err(ReadWriteError::CacheMiss) => {
                    let mut tx = self.branch().store().begin_read().await?;
                    self.blob.warmup(&mut tx).await?;
                }
                Err(ReadWriteError::CacheFull) => {
                    self.flush().await?;
                }
            }
        }
    }

    pub async fn write_all(&mut self, buffer: &[u8]) -> Result<()> {
        let mut offset = 0;

        loop {
            match self.write(&buffer[offset..]).await? {
                0 => return Ok(()),
                n => {
                    offset += n;
                }
            }
        }
    }

    /// Seeks to an offset in the file.
    pub fn seek(&mut self, pos: SeekFrom) -> u64 {
        self.blob.seek(pos)
    }

    /// Truncates the file to the given length.
    pub fn truncate(&mut self, len: u64) -> Result<()> {
        self.acquire_write_lock()?;
        self.blob.truncate(len)
    }

    /// Atomically saves any pending modifications and updates the version vectors of this file and
    /// all its ancestors.
    pub async fn flush(&mut self) -> Result<()> {
        if !self.blob.is_dirty() {
            return Ok(());
        }

        let mut tx = self.branch().store().begin_write().await?;
        let mut changeset = Changeset::new();

        self.blob.flush(&mut tx, &mut changeset).await?;
        self.parent
            .bump(
                &mut tx,
                &mut changeset,
                self.branch().clone(),
                Bump::increment(*self.branch().id()),
            )
            .await?;

        changeset
            .apply(
                &mut tx,
                self.branch().id(),
                self.branch()
                    .keys()
                    .write()
                    .ok_or(Error::PermissionDenied)?,
            )
            .await?;

        let event_tx = self.branch().notify();
        tx.commit_and_then(move || event_tx.send()).await?;

        Ok(())
    }

    /// Saves any pending modifications but does not update the version vectors. For internal use
    /// only.
    pub(crate) async fn save(
        &mut self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
    ) -> Result<()> {
        self.blob.flush(tx, changeset).await?;
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

        let lock = dst_branch.locker().read(*self.blob.id()).await;
        let lock = UpgradableLock::Read(lock);

        let blob = {
            let mut tx = dst_branch.store().begin_read().await?;
            Blob::open(&mut tx, dst_branch, *self.blob.id()).await?
        };

        *self = Self { blob, parent, lock };

        Ok(())
    }

    pub async fn version_vector(&self) -> Result<VersionVector> {
        self.parent
            .entry_version_vector(self.branch().clone())
            .await
    }

    /// BlobId of this file.
    #[cfg(test)]
    pub(crate) fn blob_id(&self) -> &crate::blob::BlobId {
        self.blob.id()
    }

    fn acquire_write_lock(&mut self) -> Result<()> {
        self.lock.upgrade().then_some(()).ok_or(Error::Locked)
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("File")
            .field("blob_id", &self.blob.id())
            .field("branch", &self.blob.branch().id())
            .finish()
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
        store::Store,
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
        file0.write_all(b"small").await.unwrap();
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
        file1.write_all(b"large").await.unwrap();
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
            file1.write_all(b"oink").await.unwrap();
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

        file0.write_all(b"yip-yap").await.unwrap();
        assert_matches!(file1.write_all(b"ring-ding-ding").await, Err(Error::Locked));
        assert_matches!(file1.truncate(0), Err(Error::Locked));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn copy_to_writer() {
        use tokio::{fs, io::AsyncReadExt};

        let (base_dir, [branch]) = setup().await;
        let src_content = b"hello world";

        let mut src = branch.ensure_file_exists("src.txt".into()).await.unwrap();
        src.write_all(src_content).await.unwrap();

        let dst_path = base_dir.path().join("dst.txt");
        let mut dst = fs::File::create(&dst_path).await.unwrap();
        src.seek(SeekFrom::Start(0));
        src.copy_to_writer(&mut dst).await.unwrap();
        drop(dst);

        let mut dst = fs::File::open(&dst_path).await.unwrap();
        let mut dst_content = Vec::new();
        dst.read_to_end(&mut dst_content).await.unwrap();

        assert_eq!(dst_content, src_content);
    }

    async fn setup<const N: usize>() -> (TempDir, [Branch; N]) {
        let (base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let keys = AccessKeys::from(WriteSecrets::random());
        let event_tx = EventSender::new(1);
        let shared = BranchShared::new();

        let branches = [(); N].map(|_| {
            create_branch(
                store.clone(),
                event_tx.clone(),
                keys.clone(),
                shared.clone(),
            )
        });

        (base_dir, branches)
    }

    fn create_branch(
        store: Store,
        event_tx: EventSender,
        keys: AccessKeys,
        shared: BranchShared,
    ) -> Branch {
        let id = PublicKey::random();
        Branch::new(id, store, keys, shared, event_tx)
    }
}

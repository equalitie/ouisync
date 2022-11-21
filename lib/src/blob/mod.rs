mod cache;
mod inner;
mod open_block;
mod operations;
#[cfg(test)]
mod tests;

pub(crate) use self::{cache::BlobCache, inner::Shared};
use self::{inner::Unique, open_block::OpenBlock, operations::Operations};
use crate::{
    blob_id::BlobId, block::BlockId, branch::Branch, db, error::Error, error::Result,
    locator::Locator, sync::Mutex,
};
use std::{io::SeekFrom, mem, sync::Arc};

/// Size of the blob header in bytes.
// Using u64 instead of usize because HEADER_SIZE must be the same irrespective of whether we're on
// a 32bit or 64bit processor (if we want two such replicas to be able to sync).
pub const HEADER_SIZE: usize = mem::size_of::<u64>();

#[derive(Clone)]
pub(crate) struct Blob {
    shared: Arc<Mutex<Shared>>,
    unique: Unique,
}

impl Blob {
    /// Opens an existing blob.
    pub async fn open(
        conn: &mut db::Connection,
        branch: Branch,
        head_locator: Locator,
        shared: Arc<Mutex<Shared>>,
    ) -> Result<Self> {
        let mut current_block = OpenBlock::open_head(conn, &branch, head_locator).await?;
        let len = current_block.content.read_u64();

        Ok(Self {
            shared,
            unique: Unique {
                branch,
                head_locator,
                current_block,
                len,
                len_dirty: false,
            },
        })
    }

    /// Creates a new blob.
    pub fn create(branch: Branch, head_locator: Locator, shared: Arc<Mutex<Shared>>) -> Self {
        let current_block = OpenBlock::new_head(head_locator);

        Self {
            shared,
            unique: Unique {
                branch,
                head_locator,
                current_block,
                len: 0,
                len_dirty: false,
            },
        }
    }

    /// Removes a blob.
    pub async fn remove(
        conn: &mut db::Connection,
        branch: &Branch,
        head_locator: Locator,
    ) -> Result<()> {
        let mut tx = conn.begin().await?;
        match operations::remove_blocks(&mut tx, branch, head_locator.sequence()).await {
            // `EntryNotFound` means we reached the end of the blob. This is expected (in fact,
            // we should never get `Ok` because the locator sequence we are passing to
            // `remove_blocks` is unbounded).
            Ok(()) | Err(Error::EntryNotFound) => (),
            Err(error) => return Err(error),
        }
        tx.commit().await?;

        Ok(())
    }

    pub fn branch(&self) -> &Branch {
        &self.unique.branch
    }

    /// Locator of this blob.
    pub fn locator(&self) -> &Locator {
        &self.unique.head_locator
    }

    pub fn len(&self) -> u64 {
        self.unique.len
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()` if the portion of the
    /// blob past the internal cursor is smaller than `buffer.len()`.
    pub async fn read(&mut self, conn: &mut db::Connection, buffer: &mut [u8]) -> Result<usize> {
        self.lock().await.read(conn, buffer).await
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self, conn: &mut db::Connection) -> Result<Vec<u8>> {
        self.lock().await.read_to_end(conn).await
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, conn: &mut db::Connection, buffer: &[u8]) -> Result<()> {
        // Always begin transactions before acquiring the lock, to avoid deadlocks.
        let mut tx = conn.begin().await?;
        self.lock().await.write(&mut tx, buffer).await?;
        tx.commit().await?;

        Ok(())
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub async fn seek(&mut self, conn: &mut db::Connection, pos: SeekFrom) -> Result<u64> {
        self.lock().await.seek(conn, pos).await
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, conn: &mut db::Connection, len: u64) -> Result<()> {
        // Always begin transactions before acquiring the lock, to avoid deadlocks.
        let mut tx = conn.begin().await?;
        self.lock().await.truncate(&mut tx, len).await?;
        tx.commit().await?;

        Ok(())
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self, conn: &mut db::Connection) -> Result<bool> {
        // Always begin transactions before acquiring the lock, to avoid deadlocks.
        let mut tx = conn.begin().await?;
        let was_dirty = self.lock().await.flush(&mut tx).await?;
        tx.commit().await?;

        Ok(was_dirty)
    }

    /// Creates a shallow copy (only the index nodes are copied, not blocks) of this blob into the
    /// specified destination branch. This function is idempotent.
    pub async fn try_fork(&self, tx: &mut db::Transaction<'_>, dst_branch: Branch) -> Result<Self> {
        // If the blob is already forked, do nothing but still return a clone of the original blob
        // to maintain idempotency.
        if self.unique.branch.id() != dst_branch.id() {
            let read_key = self.unique.branch.keys().read();
            // Take the write key from the dst branch, not the src branch, to protect us against
            // accidentally forking into remote branch (remote branches don't have write access).
            let write_keys = dst_branch.keys().write().ok_or(Error::PermissionDenied)?;

            let locators = self
                .unique
                .head_locator
                .sequence()
                .take(self.unique.block_count() as usize);

            for locator in locators {
                let encoded_locator = locator.encode(read_key);

                let block_id = self.unique.branch.data().get(tx, &encoded_locator).await?;

                // It can happen that the current and dst branches are different, but the blob has
                // already been forked by some other task in the meantime (after we loaded this
                // blob but before the `tx` transaction has been created). In that case this
                // `insert` is a no-op. We still proceed normally to maintain idempotency.
                dst_branch
                    .data()
                    .insert(tx, &block_id, &encoded_locator, write_keys)
                    .await?;
            }
        }

        let forked = Self {
            shared: dst_branch.fetch_blob_shared(*self.unique.head_locator.blob_id()),
            unique: Unique {
                branch: dst_branch,
                head_locator: self.unique.head_locator,
                current_block: self.unique.current_block.clone(),
                len: self.unique.len,
                len_dirty: self.unique.len_dirty,
            },
        };

        Ok(forked)
    }

    /// Was this blob modified and not flushed yet?
    pub fn is_dirty(&self) -> bool {
        self.unique.current_block.dirty || self.unique.len_dirty
    }

    async fn lock(&mut self) -> Operations<'_> {
        Operations {
            shared: self.shared.lock().await,
            unique: &mut self.unique,
        }
    }
}

/// Pseudo-stream that yields the block ids of the given blob in their sequential order.
pub(crate) struct BlockIds {
    branch: Branch,
    locator: Locator,
}

impl BlockIds {
    pub fn new(branch: Branch, blob_id: BlobId) -> Self {
        Self {
            branch,
            locator: Locator::head(blob_id),
        }
    }

    pub async fn next(&mut self, conn: &mut db::Connection) -> Result<Option<BlockId>> {
        let encoded = self.locator.encode(self.branch.keys().read());
        self.locator = self.locator.next();

        match self.branch.data().get(conn, &encoded).await {
            Ok(block_id) => Ok(Some(block_id)),
            Err(Error::EntryNotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

mod inner;
mod open_block;
mod operations;
#[cfg(test)]
mod tests;

pub(crate) use self::inner::{MaybeInitShared, Shared, UninitShared};
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

pub(crate) struct Blob {
    shared: Arc<Mutex<Shared>>,
    unique: Unique,
}

impl Blob {
    /// Opens an existing blob.
    pub async fn open(
        branch: Branch,
        head_locator: Locator,
        shared: MaybeInitShared,
    ) -> Result<Self> {
        let mut conn = branch.db_pool().acquire().await?;
        let mut current_block = OpenBlock::open_head(&mut conn, &branch, head_locator).await?;

        let len = current_block.content.read_u64();
        let shared = shared.ensure_init(len).await;

        Ok(Self {
            shared,
            unique: Unique {
                branch,
                head_locator,
                current_block,
                len_dirty: false,
            },
        })
    }

    /// Creates a new blob.
    pub fn create(branch: Branch, head_locator: Locator, shared: UninitShared) -> Self {
        let current_block = OpenBlock::new_head(head_locator);

        Self {
            shared: shared.init(),
            unique: Unique {
                branch,
                head_locator,
                current_block,
                len_dirty: false,
            },
        }
    }

    pub async fn first_block_id(&self) -> Result<BlockId> {
        let mut conn = self.db_pool().acquire().await?;

        self.unique
            .branch
            .data()
            .get(
                &mut conn,
                &self
                    .unique
                    .head_locator
                    .encode(self.unique.branch.keys().read()),
            )
            .await
    }

    pub fn branch(&self) -> &Branch {
        &self.unique.branch
    }

    /// Locator of this blob.
    pub fn locator(&self) -> &Locator {
        &self.unique.head_locator
    }

    pub async fn len(&self) -> u64 {
        self.shared.lock().await.len
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()` if the portion of the
    /// blob past the internal cursor is smaller than `buffer.len()`.
    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let mut conn = self.db_pool().acquire().await?;
        self.lock().await.read(&mut conn, buffer).await
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        let mut conn = self.db_pool().acquire().await?;
        self.lock().await.read_to_end(&mut conn).await
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, buffer: &[u8]) -> Result<()> {
        let mut tx = self.db_pool().begin().await?;
        self.lock().await.write(&mut tx, buffer).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Writes into this blob in a db transaction.
    pub async fn write_in_transaction(
        &mut self,
        tx: &mut db::Transaction<'_>,
        buffer: &[u8],
    ) -> Result<()> {
        self.lock().await.write(tx, buffer).await
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let mut tx = self.db_pool().begin().await?;
        let pos = self.lock().await.seek(&mut tx, pos).await?;
        tx.commit().await?;
        Ok(pos)
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, len: u64) -> Result<()> {
        let mut tx = self.db_pool().begin().await?;
        self.lock().await.truncate(&mut tx, len).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Truncate the blob to the given length in a db transaction.
    pub async fn truncate_in_transaction(
        &mut self,
        tx: &mut db::Transaction<'_>,
        len: u64,
    ) -> Result<()> {
        self.lock().await.truncate(tx, len).await
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    // NOTE: this is currently used only in tests. Everywhere else we use `flush_in_transaction`.
    #[cfg(test)]
    pub async fn flush(&mut self) -> Result<bool> {
        let mut tx = self.db_pool().begin().await?;
        let was_dirty = self.lock().await.flush(&mut tx).await?;
        tx.commit().await?;

        Ok(was_dirty)
    }

    /// Flushes this blob in a db transaction.
    pub async fn flush_in_transaction(&mut self, tx: &mut db::Transaction<'_>) -> Result<bool> {
        self.lock().await.flush(tx).await
    }

    /// Removes this blob.
    pub async fn remove(&mut self) -> Result<()> {
        let mut conn = self.db_pool().acquire().await?;
        self.lock().await.remove(&mut conn).await
    }

    /// Creates a shallow copy (only the index nodes are copied, not blocks) of this blob into the
    /// specified destination branch.
    pub async fn fork(&mut self, dst_branch: Branch) -> Result<()> {
        if self.unique.branch.id() == dst_branch.id() {
            return Ok(());
        }

        let mut conn = self.db_pool().acquire().await?;
        let new_self = self.lock().await.fork(&mut conn, dst_branch).await?;

        *self = new_self;

        Ok(())
    }

    /// Was this blob modified and not flushed yet?
    pub fn is_dirty(&self) -> bool {
        self.unique.current_block.dirty || self.unique.len_dirty
    }

    pub fn db_pool(&self) -> &db::Pool {
        self.unique.branch.db_pool()
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

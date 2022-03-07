#[cfg(test)]
mod tests;

mod core;
mod operations;

pub(crate) use self::core::Core;
use self::operations::Operations;
use crate::{
    block::{BlockId, BLOCK_SIZE},
    branch::Branch,
    db,
    error::Result,
    locator::Locator,
    Error,
};
use std::{
    convert::TryInto,
    io::SeekFrom,
    mem,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tokio::sync::Mutex;
use zeroize::Zeroize;

// Using u64 instead of usize because HEADER_SIZE must be the same irrespective of whether we're on
// a 32bit or 64bit processor (if we want two such replicas to be able to sync).
pub(super) const HEADER_SIZE: usize = mem::size_of::<u64>();

pub(crate) struct Blob {
    core: Arc<Mutex<Core>>,
    inner: Inner,
}

impl Blob {
    /// Opens an existing blob.
    pub async fn open(branch: Branch, head_locator: Locator) -> Result<Self> {
        let mut conn = branch.db_pool().acquire().await?;

        let mut current_block = OpenBlock::open_head(&mut conn, &branch, head_locator).await?;
        let len = current_block.content.read_u64();

        Ok(Self {
            core: Arc::new(Mutex::new(Core {
                len,
                len_dirty: false,
            })),
            inner: Inner {
                branch,
                head_locator,
                current_block,
            },
        })
    }

    pub async fn reopen(
        branch: Branch,
        head_locator: Locator,
        core: Arc<Mutex<Core>>,
    ) -> Result<Self> {
        // Make sure we acquire db connection first and lock the core second always in that order,
        // to prevent deadlocks.
        let mut conn = branch.db_pool().acquire().await?;
        let core_guard = core.lock().await;

        let current_block = match OpenBlock::open_head(&mut conn, &branch, head_locator).await {
            Ok(mut current_block) => {
                current_block.content.pos = HEADER_SIZE;
                current_block
            }
            Err(Error::EntryNotFound) if core_guard.len == 0 => OpenBlock::new_head(head_locator),
            Err(error) => return Err(error),
        };

        drop(core_guard);

        Ok(Self {
            core,
            inner: Inner {
                branch,
                head_locator,
                current_block,
            },
        })
    }

    pub async fn first_block_id(&self) -> Result<BlockId> {
        let mut conn = self.db_pool().acquire().await?;

        self.inner
            .branch
            .data()
            .get(
                &mut conn,
                &self
                    .inner
                    .head_locator
                    .encode(self.inner.branch.keys().read()),
            )
            .await
    }

    /// Creates a new blob.
    pub fn create(branch: Branch, head_locator: Locator) -> Self {
        let current_block = OpenBlock::new_head(head_locator);

        Self {
            core: Arc::new(Mutex::new(Core {
                len: 0,
                len_dirty: false,
            })),
            inner: Inner {
                branch,
                head_locator,
                current_block,
            },
        }
    }

    pub fn branch(&self) -> &Branch {
        &self.inner.branch
    }

    pub fn core(&self) -> &Arc<Mutex<Core>> {
        &self.core
    }

    /// Locator of this blob.
    pub fn locator(&self) -> &Locator {
        &self.inner.head_locator
    }

    pub async fn len(&self) -> u64 {
        self.core.lock().await.len
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
    /// specified destination branch and locator.
    pub async fn fork(&mut self, dst_branch: Branch, dst_head_locator: Locator) -> Result<()> {
        if self.inner.branch.id() == dst_branch.id() && self.inner.head_locator == dst_head_locator
        {
            return Ok(());
        }

        let mut conn = self.db_pool().acquire().await?;
        let new_self = self
            .lock()
            .await
            .fork(&mut conn, dst_branch, dst_head_locator)
            .await?;

        *self = new_self;

        Ok(())
    }

    /// Was this blob modified and not flushed yet?
    pub async fn is_dirty(&self) -> bool {
        self.inner.current_block.dirty || self.core.lock().await.len_dirty
    }

    pub fn db_pool(&self) -> &db::Pool {
        self.inner.branch.db_pool()
    }

    async fn lock(&mut self) -> Operations<'_> {
        Operations {
            core: self.core.lock().await,
            inner: &mut self.inner,
        }
    }
}

struct Inner {
    branch: Branch,
    head_locator: Locator,
    current_block: OpenBlock,
}

// Data for a block that's been loaded into memory and decrypted.
#[derive(Clone)]
pub(crate) struct OpenBlock {
    // Locator of the block.
    pub locator: Locator,
    // Id of the block.
    pub id: BlockId,
    // Decrypted content of the block wrapped in `Cursor` to track the current seek position.
    pub content: Cursor,
    // Was this block modified since the last time it was loaded from/saved to the store?
    pub dirty: bool,
}

impl OpenBlock {
    pub fn new_head(locator: Locator) -> Self {
        let mut content = Cursor::new(Buffer::new());
        content.write_u64(0); // blob length (initially zero)

        Self {
            locator,
            id: BlockId::from_content(&[]),
            content,
            dirty: true,
        }
    }

    async fn open_head(
        conn: &mut db::Connection,
        branch: &Branch,
        locator: Locator,
    ) -> Result<Self> {
        let (id, mut buffer, nonce) =
            operations::load_block(conn, branch.data(), branch.keys().read(), &locator).await?;
        operations::decrypt_block(branch.keys().read(), &nonce, &mut buffer);

        let content = Cursor::new(buffer);

        Ok(Self {
            locator,
            id,
            content,
            dirty: false,
        })
    }
}

// Buffer for keeping loaded block content and also for in-place encryption and decryption.
#[derive(Clone)]
pub(crate) struct Buffer(Box<[u8]>);

impl Buffer {
    pub fn new() -> Self {
        Self(vec![0; BLOCK_SIZE].into_boxed_slice())
    }
}

// Scramble the buffer on drop to prevent leaving decrypted data in memory past the buffer
// lifetime.
impl Drop for Buffer {
    fn drop(&mut self) {
        self.0.zeroize()
    }
}

impl Deref for Buffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Buffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Wrapper for `Buffer` with an internal position which advances when data is read from or
// written to the buffer.
#[derive(Clone)]
pub(crate) struct Cursor {
    pub buffer: Buffer,
    pub pos: usize,
}

impl Cursor {
    pub fn new(buffer: Buffer) -> Self {
        Self { buffer, pos: 0 }
    }

    // Reads data from the buffer into `dst` and advances the internal position. Returns the
    // number of bytes actual read.
    pub fn read(&mut self, dst: &mut [u8]) -> usize {
        let n = (self.buffer.len() - self.pos).min(dst.len());
        dst[..n].copy_from_slice(&self.buffer[self.pos..self.pos + n]);
        self.pos += n;
        n
    }

    // Read data from the buffer into a fixed-length array.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `N`.
    pub fn read_array<const N: usize>(&mut self) -> [u8; N] {
        let array = self.buffer[self.pos..self.pos + N].try_into().unwrap();
        self.pos += N;
        array
    }

    // Read data from the buffer into a `u64`.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    pub fn read_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.read_array())
    }

    // Writes data from `dst` into the buffer and advances the internal position. Returns the
    // number of bytes actually written.
    pub fn write(&mut self, src: &[u8]) -> usize {
        let n = (self.buffer.len() - self.pos).min(src.len());
        self.buffer[self.pos..self.pos + n].copy_from_slice(&src[..n]);
        self.pos += n;
        n
    }

    // Write a `u64` into the buffer.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    pub fn write_u64(&mut self, value: u64) {
        let bytes = value.to_le_bytes();
        assert!(self.buffer.len() - self.pos >= bytes.len());
        self.write(&bytes[..]);
    }
}

impl Deref for Cursor {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.buffer[self.pos..]
    }
}

impl DerefMut for Cursor {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buffer[self.pos..]
    }
}

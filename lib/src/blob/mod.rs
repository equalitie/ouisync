#[cfg(test)]
mod tests;

mod core;
mod operations;

pub(crate) use self::core::Core;
use self::operations::Operations;
use crate::{
    blob_id::BlobId,
    block::{BlockId, BLOCK_SIZE},
    branch::Branch,
    db,
    error::Result,
    locator::Locator,
};
use std::{
    convert::TryInto,
    io::SeekFrom,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tokio::sync::{Mutex, MutexGuard};
use zeroize::Zeroize;

pub(crate) struct Blob {
    core: Arc<Mutex<Core>>,
    head_locator: Locator,
    branch: Branch,
    current_block: OpenBlock,
}

impl Blob {
    pub fn new(
        core: Arc<Mutex<Core>>,
        head_locator: Locator,
        branch: Branch,
        current_block: OpenBlock,
    ) -> Self {
        Self {
            core,
            head_locator,
            branch,
            current_block,
        }
    }

    /// Opens an existing blob.
    pub async fn open(branch: Branch, head_locator: Locator) -> Result<Self> {
        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = branch.db_pool().begin().await?;

        let (id, buffer, auth_tag) =
            operations::load_block(&mut tx, branch.data(), &branch.keys().read, &head_locator)
                .await?;

        let mut content = Cursor::new(buffer);

        let nonce: BlobNonce = content.read_array();
        let blob_key = branch.keys().read.derive_subkey(&nonce);

        operations::decrypt_block(&blob_key, &id, 0, &mut content, &auth_tag)?;

        let len = content.read_u64();

        Ok(Self::new(
            Arc::new(Mutex::new(Core {
                branch: branch.clone(),
                head_locator,
                blob_key,
                len,
                len_dirty: false,
            })),
            head_locator,
            branch,
            OpenBlock {
                locator: head_locator,
                id,
                content,
                dirty: false,
            },
        ))
    }

    pub async fn first_block_id(&self) -> Result<BlockId> {
        Core::first_block_id(&self.branch, self.head_locator).await
    }

    /// Creates a new blob.
    pub fn create(branch: Branch, head_locator: Locator) -> Self {
        let nonce: BlobNonce = rand::random();
        let blob_key = branch.keys().read.derive_subkey(&nonce);

        let current_block = OpenBlock::new_head(head_locator, &nonce);

        Self::new(
            Arc::new(Mutex::new(Core {
                branch: branch.clone(),
                head_locator,
                blob_key,
                len: 0,
                len_dirty: false,
            })),
            head_locator,
            branch,
            current_block,
        )
    }

    pub async fn reopen(core: Arc<Mutex<Core>>) -> Result<Self> {
        let ptr = core.clone();
        let mut guard = core.lock().await;
        let core = &mut *guard;

        let current_block = core.open_first_block().await?;

        Ok(Self::new(
            ptr,
            core.head_locator,
            core.branch.clone(),
            current_block,
        ))
    }

    pub fn branch(&self) -> &Branch {
        &self.branch
    }

    pub fn core(&self) -> &Arc<Mutex<Core>> {
        &self.core
    }

    /// Locator of this blob.
    pub fn locator(&self) -> &Locator {
        &self.head_locator
    }

    pub fn blob_id(&self) -> &BlobId {
        self.head_locator.blob_id()
    }

    pub async fn len(&self) -> u64 {
        self.core.lock().await.len()
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()` if the portion of the
    /// blob past the internal cursor is smaller than `buffer.len()`.
    pub async fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.lock().await.ops().read(buffer).await
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        self.lock().await.ops().read_to_end().await
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, buffer: &[u8]) -> Result<()> {
        self.lock().await.ops().write(buffer).await
    }

    /// Writes into this blob in a db transaction.
    pub async fn write_in_transaction(
        &mut self,
        tx: &mut db::Transaction<'_>,
        buffer: &[u8],
    ) -> Result<()> {
        self.lock()
            .await
            .ops()
            .write_in_transaction(tx, buffer)
            .await
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        self.lock().await.ops().seek(pos).await
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, len: u64) -> Result<()> {
        self.lock().await.ops().truncate(len).await
    }

    /// Truncate the blob to the given length in a db transaction.
    pub async fn truncate_in_transaction(
        &mut self,
        tx: &mut db::Transaction<'_>,
        len: u64,
    ) -> Result<()> {
        self.lock()
            .await
            .ops()
            .truncate_in_transaction(tx, len)
            .await
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    // NOTE: this is currently used only in tests. Everywhere else we use `flush_in_transaction`.
    #[cfg(test)]
    pub async fn flush(&mut self) -> Result<bool> {
        let mut tx = self.db_pool().begin().await?;
        let was_dirty = self.flush_in_transaction(&mut tx).await?;
        tx.commit().await?;

        Ok(was_dirty)
    }

    /// Flushes this blob in a db transaction.
    pub async fn flush_in_transaction(&mut self, tx: &mut db::Transaction<'_>) -> Result<bool> {
        self.lock().await.ops().flush_in_transaction(tx).await
    }

    /// Removes this blob.
    pub async fn remove(&mut self) -> Result<()> {
        self.lock().await.ops().remove().await
    }

    /// Creates a shallow copy (only the index nodes are copied, not blocks) of this blob into the
    /// specified destination branch and locator.
    pub async fn fork(&mut self, dst_branch: Branch, dst_head_locator: Locator) -> Result<()> {
        if self.branch.id() == dst_branch.id() && self.head_locator == dst_head_locator {
            return Ok(());
        }

        let new_self = self
            .lock()
            .await
            .ops()
            .fork(dst_branch, dst_head_locator)
            .await?;

        *self = new_self;

        Ok(())
    }

    /// Was this blob modified and not flushed yet?
    pub async fn is_dirty(&self) -> bool {
        self.current_block.dirty || self.core.lock().await.len_dirty
    }

    pub fn db_pool(&self) -> &db::Pool {
        self.branch.db_pool()
    }

    async fn lock(&mut self) -> OperationsLock<'_> {
        let core_guard = self.core.lock().await;
        OperationsLock {
            core_guard,
            current_block: &mut self.current_block,
        }
    }
}

struct OperationsLock<'a> {
    core_guard: MutexGuard<'a, Core>,
    current_block: &'a mut OpenBlock,
}

impl<'a> OperationsLock<'a> {
    fn ops(&mut self) -> Operations {
        self.core_guard.operations(self.current_block)
    }
}

// Unique nonce per blob, used to derive the per-blob encryption key. Should be large enough so that
// when it's randomly generated, the chance of collision is negligible.
const BLOB_NONCE_SIZE: usize = 32;
type BlobNonce = [u8; BLOB_NONCE_SIZE];

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
    pub fn new_head(locator: Locator, nonce: &BlobNonce) -> Self {
        let mut content = Cursor::new(Buffer::new());
        content.write(nonce);
        content.write_u64(0); // blob length (initially zero)

        Self {
            locator,
            id: rand::random(),
            content,
            dirty: true,
        }
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

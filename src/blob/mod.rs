#[cfg(test)]
mod tests;

use crate::{
    block::{self, BlockId, BLOCK_SIZE},
    branch::Branch,
    crypto::{AuthTag, Cryptor, Hashable, NonceSequence},
    db,
    error::{Error, Result},
    global_locator::GlobalLocator,
    index::BranchData,
    locator::Locator,
    store,
};
use std::{
    convert::TryInto,
    io::SeekFrom,
    mem,
    ops::{Deref, DerefMut},
};
use zeroize::Zeroize;

pub struct Blob {
    branch: Branch,
    locator: Locator,
    nonce_sequence: NonceSequence,
    current_block: OpenBlock,
    len: u64,
    len_dirty: bool,
}

// TODO: figure out how to implement `flush` on `Drop`.

impl Blob {
    /// Opens an existing blob.
    pub(crate) async fn open(branch: Branch, locator: Locator) -> Result<Self> {
        // NOTE: no need to commit this transaction because we are only reading here.
        let mut tx = branch.db_pool().begin().await?;

        let (id, buffer, auth_tag) =
            load_block(&mut tx, branch.data(), branch.cryptor(), &locator).await?;

        let mut content = Cursor::new(buffer);

        let nonce_sequence = NonceSequence::new(content.read_array());
        let nonce = nonce_sequence.get(0);

        branch
            .cryptor()
            .decrypt(&nonce, id.as_ref(), &mut content, &auth_tag)?;

        let len = content.read_u64();

        let current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: false,
        };

        Ok(Self {
            branch,
            locator,
            nonce_sequence,
            current_block,
            len,
            len_dirty: false,
        })
    }

    /// Creates a new blob.
    pub fn create(branch: Branch, locator: Locator) -> Self {
        let nonce_sequence = NonceSequence::new(rand::random());
        let mut content = Cursor::new(Buffer::new());

        content.write(&nonce_sequence.prefix()[..]);
        content.write_u64(0); // blob length

        let id = rand::random();
        let current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: true,
        };

        Self {
            branch,
            locator,
            nonce_sequence,
            current_block,
            len: 0,
            len_dirty: false,
        }
    }

    pub fn branch(&self) -> &Branch {
        &self.branch
    }

    /// Length of this blob in bytes.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Locator of this blob.
    pub fn locator(&self) -> &Locator {
        &self.locator
    }

    /// GlobalLocator of this blob.
    pub fn global_locator(&self) -> GlobalLocator {
        GlobalLocator {
            branch_id: *self.branch.id(),
            local: self.locator,
        }
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()` if the portion of the
    /// blob past the internal cursor is smaller than `buffer.len()`.
    pub async fn read(&mut self, mut buffer: &mut [u8]) -> Result<usize> {
        let mut total_len = 0;

        loop {
            let remaining = (self.len() - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX);
            let len = buffer.len().min(remaining);
            let len = self.current_block.content.read(&mut buffer[..len]);

            buffer = &mut buffer[len..];
            total_len += len;

            if buffer.is_empty() {
                break;
            }

            let locator = self.current_block.locator.next();
            if locator.number() >= self.block_count() {
                break;
            }

            // NOTE: unlike in `write` we create a separate transaction for each iteration. This is
            // because if we created a single transaction for the whole `read` call, then a failed
            // read could rollback the changes made in a previous iteration which would then be
            // lost. This is fine because there is going to be at most one dirty block within
            // a single `read` invocation anyway.
            let mut tx = self.db_pool().begin().await?;

            let (id, content) = read_block(
                &mut tx,
                self.branch.data(),
                self.cryptor(),
                &self.nonce_sequence,
                &locator,
            )
            .await?;

            self.replace_current_block(&mut tx, locator, id, content)
                .await?;

            tx.commit().await?;
        }

        Ok(total_len)
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self) -> Result<Vec<u8>> {
        let mut buffer = vec![
            0;
            (self.len() - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX)
        ];

        let len = self.read(&mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, mut buffer: &[u8]) -> Result<()> {
        // Wrap the whole `write` in a transaction to make it atomic.
        let mut tx = self.db_pool().begin().await?;

        loop {
            let len = self.current_block.content.write(buffer);

            // TODO: only set the dirty flag if the content actually changed. Otherwise overwirting
            // a block with the same content it already had would result in a new block with a new
            // version being unnecessarily created.
            if len > 0 {
                self.current_block.dirty = true;
            }

            buffer = &buffer[len..];

            if self.seek_position() > self.len {
                self.len = self.seek_position();
                self.len_dirty = true;
            }

            if buffer.is_empty() {
                break;
            }

            let locator = self.current_block.locator.next();
            let (id, content) = if locator.number() < self.block_count() {
                read_block(
                    &mut tx,
                    self.branch.data(),
                    self.cryptor(),
                    &self.nonce_sequence,
                    &locator,
                )
                .await?
            } else {
                (rand::random(), Buffer::new())
            };

            self.replace_current_block(&mut tx, locator, id, content)
                .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub async fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let offset = match pos {
            SeekFrom::Start(n) => n.min(self.len),
            SeekFrom::End(n) => {
                if n >= 0 {
                    self.len
                } else {
                    self.len.saturating_sub((-n) as u64)
                }
            }
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.seek_position().saturating_add(n as u64).min(self.len)
                } else {
                    self.seek_position().saturating_sub((-n) as u64)
                }
            }
        };

        let actual_offset = offset + self.header_size() as u64;
        let block_number = (actual_offset / BLOCK_SIZE as u64) as u32;
        let block_offset = (actual_offset % BLOCK_SIZE as u64) as usize;

        if block_number != self.current_block.locator.number() {
            let locator = self.locator_at(block_number);

            let mut tx = self.db_pool().begin().await?;
            let (id, content) = read_block(
                &mut tx,
                self.branch.data(),
                self.cryptor(),
                &self.nonce_sequence,
                &locator,
            )
            .await?;
            self.replace_current_block(&mut tx, locator, id, content)
                .await?;
            tx.commit().await?;
        }

        self.current_block.content.pos = block_offset;

        Ok(offset)
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, len: u64) -> Result<()> {
        // TODO: reuse the truncated blocks on subsequent writes if the content is identical

        if len == self.len {
            return Ok(());
        }

        if len > self.len {
            // TODO: consider supporting this
            return Err(Error::OperationNotSupported);
        }

        let old_block_count = self.block_count();

        self.len = len;
        self.len_dirty = true;

        let new_block_count = self.block_count();

        if self.seek_position() > self.len {
            self.seek(SeekFrom::End(0)).await?;
        }

        self.remove_blocks(
            self.locator()
                .sequence()
                .skip(new_block_count as usize)
                .take((old_block_count - new_block_count) as usize),
        )
        .await
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self) -> Result<()> {
        let mut tx = self.db_pool().begin().await?;
        self.write_len(&mut tx).await?;
        self.write_current_block(&mut tx).await?;
        tx.commit().await?;

        Ok(())
    }

    /// Removes this blob.
    pub async fn remove(self) -> Result<()> {
        self.remove_blocks(self.locator().sequence().take(self.block_count() as usize))
            .await
    }

    /// Creates a shallow copy (only the index nodes are copied, not blocks) of this blob into the
    /// specified destination branch and locator.
    pub async fn fork(&mut self, dst_branch: BranchData, dst_head_locator: Locator) -> Result<()> {
        if self.branch.id() == dst_branch.replica_id() && self.locator == dst_head_locator {
            // This blob is already in the dst, nothing to do.
            return Ok(());
        }

        let mut tx = self.db_pool().begin().await?;

        for (src_locator, dst_locator) in self.locators().zip(dst_head_locator.sequence()) {
            let encoded_src_locator = src_locator.encode(self.cryptor());
            let encoded_dst_locator = dst_locator.encode(self.cryptor());

            let block_id = self
                .branch
                .data()
                .get(&mut tx, &encoded_src_locator)
                .await?;
            dst_branch
                .insert(&mut tx, &block_id, &encoded_dst_locator)
                .await?;
        }

        tx.commit().await?;

        // TODO: the following lines must happen atomically (either all succeed or none). There is
        // currently no reason why they wouldn't, but to be super extra sure, consider wrapping
        // them in `catch_unwind`.
        self.branch = Branch::new(self.db_pool().clone(), dst_branch, self.cryptor().clone());
        self.locator = dst_head_locator;
        self.current_block.locator = self.locator_at(self.current_block.locator.number());

        Ok(())
    }

    pub fn db_pool(&self) -> &db::Pool {
        self.branch.db_pool()
    }

    pub fn cryptor(&self) -> &Cryptor {
        self.branch.cryptor()
    }

    pub fn locators(&self) -> impl Iterator<Item = Locator> {
        self.locator().sequence().take(self.block_count() as usize)
    }

    async fn replace_current_block(
        &mut self,
        tx: &mut db::Transaction<'_>,
        locator: Locator,
        id: BlockId,
        content: Buffer,
    ) -> Result<()> {
        self.write_len(tx).await?;
        self.write_current_block(tx).await?;

        let mut content = Cursor::new(content);

        if locator.number() == 0 {
            // If head block, skip over the header.
            content.pos = self.header_size();
        }

        self.current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: false,
        };

        Ok(())
    }

    // Write the current block into the store.
    async fn write_current_block(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if !self.current_block.dirty {
            return Ok(());
        }

        self.current_block.id = rand::random();

        write_block(
            tx,
            self.branch.data(),
            self.cryptor(),
            &self.nonce_sequence,
            &self.current_block.locator,
            &self.current_block.id,
            self.current_block.content.buffer.clone(),
        )
        .await?;

        self.current_block.dirty = false;

        Ok(())
    }

    // Write the current blob length into the blob header in the head block.
    async fn write_len(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if !self.len_dirty {
            return Ok(());
        }

        if self.current_block.locator.number() == 0 {
            let old_pos = self.current_block.content.pos;
            self.current_block.content.pos = self.nonce_sequence.prefix().len();
            self.current_block.content.write_u64(self.len);
            self.current_block.content.pos = old_pos;
            self.current_block.dirty = true;
        } else {
            let locator = self.locator_at(0);
            let (_, buffer) = read_block(
                tx,
                self.branch.data(),
                self.cryptor(),
                &self.nonce_sequence,
                &locator,
            )
            .await?;

            let mut cursor = Cursor::new(buffer);
            cursor.pos = self.nonce_sequence.prefix().len();
            cursor.write_u64(self.len);

            write_block(
                tx,
                self.branch.data(),
                self.cryptor(),
                &self.nonce_sequence,
                &locator,
                &rand::random(),
                cursor.buffer,
            )
            .await?;
        }

        self.len_dirty = false;

        Ok(())
    }

    async fn remove_blocks<T>(&self, locators: T) -> Result<()>
    where
        T: IntoIterator<Item = Locator>,
    {
        let mut tx = self.db_pool().begin().await?;

        for locator in locators {
            let block_id = self
                .branch
                .data()
                .remove(&mut tx, &locator.encode(self.cryptor()))
                .await?;
            store::remove_orphaned_block(&mut tx, &block_id).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    // Total number of blocks in this blob including the possibly partially filled final block.
    fn block_count(&self) -> u32 {
        // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
        (1 + (self.len + self.header_size() as u64 - 1) / BLOCK_SIZE as u64)
            .try_into()
            .unwrap_or(u32::MAX)
    }

    // Returns the current seek position from the start of the blob.
    fn seek_position(&self) -> u64 {
        self.current_block.locator.number() as u64 * BLOCK_SIZE as u64
            + self.current_block.content.pos as u64
            - self.header_size() as u64
    }

    fn header_size(&self) -> usize {
        self.nonce_sequence.prefix().len() + mem::size_of_val(&self.len)
    }

    fn locator_at(&self, number: u32) -> Locator {
        if number == 0 {
            *self.locator()
        } else {
            Locator::Trunk(self.locator().hash(), number)
        }
    }
}

async fn read_block(
    tx: &mut db::Transaction<'_>,
    branch: &BranchData,
    cryptor: &Cryptor,
    nonce_sequence: &NonceSequence,
    locator: &Locator,
) -> Result<(BlockId, Buffer)> {
    let (id, mut buffer, auth_tag) = load_block(tx, branch, cryptor, locator).await?;

    let number = locator.number();
    let nonce = nonce_sequence.get(number);
    let aad = id.as_ref(); // "additional associated data"

    let offset = if number == 0 {
        nonce_sequence.prefix().len()
    } else {
        0
    };

    cryptor.decrypt(&nonce, aad, &mut buffer[offset..], &auth_tag)?;

    Ok((id, buffer))
}

async fn load_block(
    tx: &mut db::Transaction<'_>,
    branch: &BranchData,
    cryptor: &Cryptor,
    locator: &Locator,
) -> Result<(BlockId, Buffer, AuthTag)> {
    let id = branch.get(tx, &locator.encode(cryptor)).await?;
    let mut content = Buffer::new();
    let auth_tag = block::read(tx, &id, &mut content).await?;

    Ok((id, content, auth_tag))
}

async fn write_block(
    tx: &mut db::Transaction<'_>,
    branch: &BranchData,
    cryptor: &Cryptor,
    nonce_sequence: &NonceSequence,
    locator: &Locator,
    block_id: &BlockId,
    mut buffer: Buffer,
) -> Result<()> {
    let number = locator.number();
    let nonce = nonce_sequence.get(number);
    let aad = block_id.as_ref(); // "additional associated data"

    let offset = if number == 0 {
        nonce_sequence.prefix().len()
    } else {
        0
    };

    let auth_tag = cryptor.encrypt(&nonce, aad, &mut buffer[offset..])?;

    block::write(tx, block_id, &buffer, &auth_tag).await?;
    if let Some(old_block_id) = branch
        .insert(tx, block_id, &locator.encode(cryptor))
        .await?
    {
        store::remove_orphaned_block(tx, &old_block_id).await?;
    }

    Ok(())
}

// Data for a block that's been loaded into memory and decrypted.
#[derive(Clone)]
struct OpenBlock {
    // Locator of the block.
    locator: Locator,
    // Id of the block.
    id: BlockId,
    // Decrypted content of the block wrapped in `Cursor` to track the current seek position.
    content: Cursor,
    // Was this block modified since the last time it was loaded from/saved to the store?
    dirty: bool,
}

// Buffer for keeping loaded block content and also for in-place encryption and decryption.
#[derive(Clone)]
struct Buffer(Box<[u8]>);

impl Buffer {
    fn new() -> Self {
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
struct Cursor {
    buffer: Buffer,
    pos: usize,
}

impl Cursor {
    fn new(buffer: Buffer) -> Self {
        Self { buffer, pos: 0 }
    }

    // Reads data from the buffer into `dst` and advances the internal position. Returns the
    // number of bytes actual read.
    fn read(&mut self, dst: &mut [u8]) -> usize {
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
    fn read_array<const N: usize>(&mut self) -> [u8; N] {
        let array = self.buffer[self.pos..self.pos + N].try_into().unwrap();
        self.pos += N;
        array
    }

    // Read data from the buffer into a `u64`.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    fn read_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.read_array())
    }

    // Writes data from `dst` into the buffer and advances the internal position. Returns the
    // number of bytes actually written.
    fn write(&mut self, src: &[u8]) -> usize {
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
    fn write_u64(&mut self, value: u64) {
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

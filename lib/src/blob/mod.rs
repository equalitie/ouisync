mod cache;
mod inner;
mod open_block;
#[cfg(test)]
mod tests;

pub(crate) use self::{cache::BlobCache, inner::Shared};
use self::{
    inner::Unique,
    open_block::{Buffer, Cursor, OpenBlock},
};
use crate::{
    blob_id::BlobId,
    block::{self, BlockId, BlockNonce, BLOCK_SIZE},
    branch::Branch,
    crypto::{
        cipher::{self, Nonce, SecretKey},
        sign,
    },
    db,
    error::{Error, Result},
    index::BranchData,
    locator::Locator,
};
use std::{io::SeekFrom, mem, sync::Arc};

/// Size of the blob header in bytes.
// Using u64 instead of usize because HEADER_SIZE must be the same irrespective of whether we're on
// a 32bit or 64bit processor (if we want two such replicas to be able to sync).
pub const HEADER_SIZE: usize = mem::size_of::<u64>();

#[derive(Clone)]
pub(crate) struct Blob {
    shared: Arc<Shared>,
    unique: Unique,
}

impl Blob {
    /// Opens an existing blob.
    pub async fn open(
        conn: &mut db::Connection,
        branch: Branch,
        head_locator: Locator,
        shared: Arc<Shared>,
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
    pub fn create(branch: Branch, head_locator: Locator, shared: Arc<Shared>) -> Self {
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
        match remove_blocks(&mut tx, branch, head_locator.sequence()).await {
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
    pub async fn read(
        &mut self,
        conn: &mut db::Connection,
        mut buffer: &mut [u8],
    ) -> Result<usize> {
        let mut total_len = 0;

        loop {
            let remaining = (self.unique.len - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX);
            let len = buffer.len().min(remaining);
            let len = self.unique.current_block.content.read(&mut buffer[..len]);

            buffer = &mut buffer[len..];
            total_len += len;

            if buffer.is_empty() {
                break;
            }

            let locator = self.unique.current_block.locator.next();
            if locator.number() >= self.unique.block_count() {
                break;
            }

            let (id, content) = read_block(
                conn,
                self.unique.branch.data(),
                self.unique.branch.keys().read(),
                &locator,
            )
            .await?;

            self.replace_current_block(locator, id, content)?;
        }

        Ok(total_len)
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self, conn: &mut db::Connection) -> Result<Vec<u8>> {
        let mut buffer = vec![
            0;
            (self.unique.len - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX)
        ];

        let len = self.read(conn, &mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, conn: &mut db::Connection, mut buffer: &[u8]) -> Result<()> {
        let mut tx = conn.begin().await?;

        loop {
            let len = self.unique.current_block.content.write(buffer);

            // TODO: only set the dirty flag if the content actually changed. Otherwise overwriting
            // a block with the same content it already had would result in a new block with a new
            // version being unnecessarily created.
            if len > 0 {
                self.unique.current_block.dirty = true;
            }

            buffer = &buffer[len..];

            if self.seek_position() > self.unique.len {
                self.unique.len = self.seek_position();
                self.unique.len_dirty = true;
            }

            if buffer.is_empty() {
                break;
            }

            let locator = self.unique.current_block.locator.next();
            let (id, content) = if locator.number() < self.unique.block_count() {
                read_block(
                    &mut tx,
                    self.unique.branch.data(),
                    self.unique.branch.keys().read(),
                    &locator,
                )
                .await?
            } else {
                (BlockId::from_content(&[]), Buffer::new())
            };

            self.write_current_block(&mut tx).await?;
            self.replace_current_block(locator, id, content)?;
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
    pub async fn seek(&mut self, conn: &mut db::Connection, pos: SeekFrom) -> Result<u64> {
        let offset = match pos {
            SeekFrom::Start(n) => n.min(self.unique.len),
            SeekFrom::End(n) => {
                if n >= 0 {
                    self.unique.len
                } else {
                    self.unique.len.saturating_sub((-n) as u64)
                }
            }
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.seek_position()
                        .saturating_add(n as u64)
                        .min(self.unique.len)
                } else {
                    self.seek_position().saturating_sub((-n) as u64)
                }
            }
        };

        let actual_offset = offset + HEADER_SIZE as u64;
        let block_number = (actual_offset / BLOCK_SIZE as u64) as u32;
        let block_offset = (actual_offset % BLOCK_SIZE as u64) as usize;

        if block_number != self.unique.current_block.locator.number() {
            let locator = self.unique.locator_at(block_number);

            let (id, content) = read_block(
                conn,
                self.unique.branch.data(),
                self.unique.branch.keys().read(),
                &locator,
            )
            .await?;

            self.replace_current_block(locator, id, content)?;
        }

        self.unique.current_block.content.pos = block_offset;

        Ok(offset)
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, conn: &mut db::Connection, len: u64) -> Result<()> {
        if len == self.unique.len {
            return Ok(());
        }

        if len > self.unique.len {
            // TODO: consider supporting this
            return Err(Error::OperationNotSupported);
        }

        let mut tx = conn.begin().await?;

        let old_block_count = self.unique.block_count();

        // FIXME: this is not cancel-safe. We should update `self` only after the transaction is
        // committed.

        self.unique.len = len;
        self.unique.len_dirty = true;

        let new_block_count = self.unique.block_count();

        if self.seek_position() > self.unique.len {
            self.seek(&mut tx, SeekFrom::End(0)).await?;
        }

        remove_blocks(
            &mut tx,
            &self.unique.branch,
            self.unique
                .head_locator
                .sequence()
                .skip(new_block_count as usize)
                .take((old_block_count - new_block_count) as usize),
        )
        .await?;

        tx.commit().await?;

        Ok(())
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self, conn: &mut db::Connection) -> Result<bool> {
        if !self.is_dirty() {
            return Ok(false);
        }

        let mut tx = conn.begin().await?;

        self.write_len(&mut tx).await?;
        self.write_current_block(&mut tx).await?;

        tx.commit().await?;

        Ok(true)
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

    // Returns the current seek position from the start of the blob.
    fn seek_position(&self) -> u64 {
        self.unique.current_block.locator.number() as u64 * BLOCK_SIZE as u64
            + self.unique.current_block.content.pos as u64
            - HEADER_SIZE as u64
    }

    // Precondition: the current block is not dirty (panics if it is).
    fn replace_current_block(
        &mut self,
        locator: Locator,
        id: BlockId,
        content: Buffer,
    ) -> Result<()> {
        // Prevent data loss
        // TODO: Find a way to write the current dirty block to the db in a deadlock-free way.
        if self.unique.current_block.dirty {
            return Err(Error::OperationNotSupported);
        }

        let mut content = Cursor::new(content);

        if locator.number() == 0 {
            // If head block, skip over the header.
            content.pos = HEADER_SIZE;
        }

        self.unique.current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: false,
        };

        Ok(())
    }

    // Write the current blob length into the blob header in the head block.
    async fn write_len(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if !self.unique.len_dirty {
            return Ok(());
        }

        let read_key = self.unique.branch.keys().read();
        let write_keys = self
            .unique
            .branch
            .keys()
            .write()
            .ok_or(Error::PermissionDenied)?;

        if self.unique.current_block.locator.number() == 0 {
            let old_pos = self.unique.current_block.content.pos;
            self.unique.current_block.content.pos = 0;
            self.unique.current_block.content.write_u64(self.unique.len);
            self.unique.current_block.content.pos = old_pos;
            self.unique.current_block.dirty = true;
        } else {
            let locator = self.unique.locator_at(0);
            let (_, buffer) = read_block(
                tx,
                self.unique.branch.data(),
                self.unique.branch.keys().read(),
                &locator,
            )
            .await?;

            let mut cursor = Cursor::new(buffer);
            cursor.pos = 0;
            cursor.write_u64(self.unique.len);

            write_block(
                tx,
                self.unique.branch.data(),
                read_key,
                write_keys,
                &locator,
                cursor.buffer,
            )
            .await?;
        }

        self.unique.len_dirty = false;

        Ok(())
    }

    // Write the current block into the store.
    async fn write_current_block(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if !self.unique.current_block.dirty {
            return Ok(());
        }

        let read_key = self.unique.branch.keys().read();
        let write_keys = self
            .unique
            .branch
            .keys()
            .write()
            .ok_or(Error::PermissionDenied)?;

        let block_id = write_block(
            tx,
            self.unique.branch.data(),
            read_key,
            write_keys,
            &self.unique.current_block.locator,
            self.unique.current_block.content.buffer.clone(),
        )
        .await?;

        self.unique.current_block.id = block_id;
        self.unique.current_block.dirty = false;

        Ok(())
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

async fn read_block(
    conn: &mut db::Connection,
    branch: &BranchData,
    read_key: &cipher::SecretKey,
    locator: &Locator,
) -> Result<(BlockId, Buffer)> {
    let (id, mut buffer, nonce) = load_block(conn, branch, read_key, locator).await?;
    decrypt_block(read_key, &nonce, &mut buffer);

    Ok((id, buffer))
}

async fn load_block(
    conn: &mut db::Connection,
    branch: &BranchData,
    read_key: &cipher::SecretKey,
    locator: &Locator,
) -> Result<(BlockId, Buffer, BlockNonce)> {
    // TODO: there is a small (but non-zero) chance that the block might be deleted in between the
    // `BranchData::get` and `block::read` calls by another thread. To avoid this, we should make
    // the two operations atomic. This can be done either by running them in a transaction or by
    // rewriting them as a single query (which can be done using "recursive common table
    // expressions" (https://www.sqlite.org/lang_with.html)). If we decide to use transactions, we
    // need to be extra careful to not cause deadlock due the order tables are locked when shared
    // cache is enabled. The recursive query seems like the safer option.
    //
    // NOTE: the above comment was written when we were still using multiple db connections. Now
    // with just one connection this function has exclusive access to the db and so the described
    // situation is impossible. Leaving it here in case we ever go back to multiple connections.

    let id = branch.get(conn, &locator.encode(read_key)).await?;
    let mut content = Buffer::new();
    let nonce = block::read(conn, &id, &mut content).await?;

    Ok((id, content, nonce))
}

async fn write_block(
    tx: &mut db::Transaction<'_>,
    branch: &BranchData,
    read_key: &cipher::SecretKey,
    write_keys: &sign::Keypair,
    locator: &Locator,
    mut buffer: Buffer,
) -> Result<BlockId> {
    let nonce = rand::random();
    encrypt_block(read_key, &nonce, &mut buffer);
    let id = BlockId::from_content(&buffer);

    // NOTE: make sure the index and block store operations run in the same order as in
    // `load_block` to prevent potential deadlocks when `load_block` and `write_block` run
    // concurrently and `load_block` runs inside a transaction.
    let inserted = branch
        .insert(tx, &id, &locator.encode(read_key), write_keys)
        .await?;

    // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
    // hit one in 2^sizeof(BlockId) chance that we randomly generated the same BlockId twice.
    assert!(inserted);

    block::write(tx, &id, &buffer, &nonce).await?;

    Ok(id)
}

fn decrypt_block(blob_key: &cipher::SecretKey, block_nonce: &BlockNonce, content: &mut [u8]) {
    let block_key = SecretKey::derive_from_key(blob_key.as_ref(), block_nonce);
    block_key.decrypt_no_aead(&Nonce::default(), content);
}

fn encrypt_block(blob_key: &cipher::SecretKey, block_nonce: &BlockNonce, content: &mut [u8]) {
    let block_key = SecretKey::derive_from_key(blob_key.as_ref(), block_nonce);
    block_key.encrypt_no_aead(&Nonce::default(), content);
}

async fn remove_blocks<T>(tx: &mut db::Transaction<'_>, branch: &Branch, locators: T) -> Result<()>
where
    T: IntoIterator<Item = Locator>,
{
    let read_key = branch.keys().read();
    let write_keys = branch.keys().write().ok_or(Error::PermissionDenied)?;

    for locator in locators {
        branch
            .data()
            .remove(tx, &locator.encode(read_key), write_keys)
            .await?;
    }

    Ok(())
}

// async fn read_len(conn: &mut db::Connection, unique: &Unique) -> Result<u64> {
//     if unique.current_block.locator.number() == 0 {
//         Ok(unique.current_block.content.buffer.read_u64(0))
//     } else {
//         let locator = unique.locator_at(0);
//         let (_, buffer) = read_block(
//             conn,
//             unique.branch.data(),
//             unique.branch.keys().read(),
//             &locator,
//         )
//         .await?;

//         Ok(buffer.read_u64(0))
//     }
// }

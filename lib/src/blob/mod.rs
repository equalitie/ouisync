pub(crate) mod lock;

mod block_ids;
mod open_block;
#[cfg(test)]
mod tests;

pub(crate) use self::block_ids::BlockIds;
use self::open_block::{Buffer, Cursor, OpenBlock};
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
    index::{SingleBlockPresence, SnapshotData},
    locator::Locator,
};
use std::{io::SeekFrom, mem};
use tracing::{field, instrument, Instrument, Span};

/// Size of the blob header in bytes.
// Using u64 instead of usize because HEADER_SIZE must be the same irrespective of whether we're on
// a 32bit or 64bit processor (if we want two such replicas to be able to sync).
pub const HEADER_SIZE: usize = mem::size_of::<u64>();

#[derive(Clone)]
pub(crate) struct Blob {
    branch: Branch,
    head_locator: Locator,
    current_block: OpenBlock,
    len: u64,
    len_dirty: bool,
}

impl Blob {
    /// Opens an existing blob.
    pub async fn open(
        tx: &mut db::ReadTransaction,
        branch: Branch,
        head_locator: Locator,
    ) -> Result<Self> {
        let snapshot = branch.data().load_snapshot(tx).await?;
        Self::open_in(tx, branch, &snapshot, head_locator).await
    }

    /// Opens an existing blob in the given snapshot
    pub async fn open_in(
        tx: &mut db::ReadTransaction,
        branch: Branch,
        snapshot: &SnapshotData,
        head_locator: Locator,
    ) -> Result<Self> {
        assert_eq!(branch.id(), snapshot.branch_id());

        let mut current_block =
            OpenBlock::open_head(tx, snapshot, branch.keys().read(), head_locator).await?;
        let len = current_block.content.read_u64();

        Ok(Self {
            branch,
            head_locator,
            current_block,
            len,
            len_dirty: false,
        })
    }

    /// Creates a new blob.
    pub fn create(branch: Branch, head_locator: Locator) -> Self {
        let current_block = OpenBlock::new_head(head_locator);

        Self {
            branch,
            head_locator,
            current_block,
            len: 0,
            len_dirty: false,
        }
    }

    pub fn branch(&self) -> &Branch {
        &self.branch
    }

    /// Locator of this blob.
    pub fn locator(&self) -> &Locator {
        &self.head_locator
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()` if the portion of the
    /// blob past the internal cursor is smaller than `buffer.len()`.
    pub async fn read(&mut self, tx: &mut db::ReadTransaction, buffer: &mut [u8]) -> Result<usize> {
        let snapshot = self.branch.data().load_snapshot(tx).await?;
        self.read_in(tx, &snapshot, buffer).await
    }

    /// Reads data from this blob in the given snapshot.
    pub async fn read_in(
        &mut self,
        tx: &mut db::ReadTransaction,
        snapshot: &SnapshotData,
        mut buffer: &mut [u8],
    ) -> Result<usize> {
        assert_eq!(self.branch.id(), snapshot.branch_id());

        let mut total_len = 0;

        loop {
            let remaining = (self.len - self.seek_position())
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

            // TODO: if this returns `EntryNotFound` it might be that some other task concurrently
            // truncated this blob and we are trying to read pass its end. In that case we should
            // update `self.len` and try the loop again.
            let (id, content) =
                read_block(tx, snapshot, self.branch.keys().read(), &locator).await?;

            self.replace_current_block(locator, id, content)?;
        }

        Ok(total_len)
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self, tx: &mut db::ReadTransaction) -> Result<Vec<u8>> {
        let snapshot = self.branch.data().load_snapshot(tx).await?;
        self.read_to_end_in(tx, &snapshot).await
    }

    /// Read all data from this blob in the given snapshot.
    pub async fn read_to_end_in(
        &mut self,
        tx: &mut db::ReadTransaction,
        snapshot: &SnapshotData,
    ) -> Result<Vec<u8>> {
        assert_eq!(self.branch.id(), snapshot.branch_id());

        let mut buffer = vec![
            0;
            (self.len - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX)
        ];

        let len = self.read_in(tx, snapshot, &mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    /// Returns whether at least one new block was written into the block store dring this
    /// operation.
    pub async fn write(
        &mut self,
        tx: &mut db::WriteTransaction,
        mut buffer: &[u8],
    ) -> Result<bool> {
        let mut snapshot = None;
        let mut block_written = false;

        loop {
            let len = self.current_block.content.write(buffer);

            // TODO: only set the dirty flag if the content actually changed. Otherwise overwriting
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

            let snapshot = if let Some(snapshot) = &mut snapshot {
                snapshot
            } else {
                let write_keys = self.branch.keys().write().ok_or(Error::PermissionDenied)?;
                snapshot.get_or_insert(
                    self.branch
                        .data()
                        .load_or_create_snapshot(tx, write_keys)
                        .await?,
                )
            };

            let (id, content) = if locator.number() < self.block_count() {
                read_block(tx, snapshot, self.branch.keys().read(), &locator).await?
            } else {
                (BlockId::from_content(&[]), Buffer::new())
            };

            self.write_len(tx, snapshot).await?;
            self.write_current_block(tx, snapshot).await?;
            self.replace_current_block(locator, id, content)?;

            block_written = true;
        }

        Ok(block_written)
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub async fn seek(&mut self, tx: &mut db::ReadTransaction, pos: SeekFrom) -> Result<u64> {
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

        let actual_offset = offset + HEADER_SIZE as u64;
        let block_number = (actual_offset / BLOCK_SIZE as u64) as u32;
        let block_offset = (actual_offset % BLOCK_SIZE as u64) as usize;

        if block_number != self.current_block.locator.number() {
            let locator = self.head_locator.nth(block_number);
            let snapshot = self.branch.data().load_snapshot(tx).await?;

            // TODO: if this returns `EntryNotFound` it might be that some other task concurrently
            // truncated this blob and we are trying to seek pass its end. In that case we should
            // update `self.len` and try again.
            let (id, content) =
                read_block(tx, &snapshot, self.branch.keys().read(), &locator).await?;

            self.replace_current_block(locator, id, content)?;
        }

        self.current_block.content.pos = block_offset;

        Ok(offset)
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, tx: &mut db::ReadTransaction, len: u64) -> Result<()> {
        if len == self.len {
            return Ok(());
        }

        if len > self.len {
            // TODO: consider supporting this
            return Err(Error::OperationNotSupported);
        }

        if self.seek_position() > len {
            self.seek(tx, SeekFrom::Start(len)).await?;
        }

        self.len = len;
        self.len_dirty = true;

        Ok(())
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub async fn flush(&mut self, tx: &mut db::WriteTransaction) -> Result<bool> {
        if !self.is_dirty() {
            return Ok(false);
        }

        let write_keys = self.branch.keys().write().ok_or(Error::PermissionDenied)?;
        let mut snapshot = self
            .branch
            .data()
            .load_or_create_snapshot(tx, write_keys)
            .await?;

        self.write_len(tx, &mut snapshot).await?;
        self.write_current_block(tx, &mut snapshot).await?;

        Ok(true)
    }

    /// Was this blob modified and not flushed yet?
    pub fn is_dirty(&self) -> bool {
        self.current_block.dirty || self.len_dirty
    }

    // Returns the current seek position from the start of the blob.
    fn seek_position(&self) -> u64 {
        self.current_block.locator.number() as u64 * BLOCK_SIZE as u64
            + self.current_block.content.pos as u64
            - HEADER_SIZE as u64
    }

    pub fn block_count(&self) -> u32 {
        block_count(self.len)
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
        if self.current_block.dirty {
            return Err(Error::OperationNotSupported);
        }

        let mut content = Cursor::new(content);

        if locator.number() == 0 {
            // If head block, skip over the header.
            content.pos = HEADER_SIZE;
        }

        self.current_block = OpenBlock {
            locator,
            id,
            content,
            dirty: false,
        };

        Ok(())
    }

    // Write the current blob length into the blob header in the head block.
    async fn write_len(
        &mut self,
        tx: &mut db::WriteTransaction,
        snapshot: &mut SnapshotData,
    ) -> Result<()> {
        if !self.len_dirty {
            return Ok(());
        }

        let read_key = self.branch.keys().read();
        let write_keys = self.branch.keys().write().ok_or(Error::PermissionDenied)?;

        if self.current_block.locator.number() == 0 {
            let old_pos = self.current_block.content.pos;
            self.current_block.content.pos = 0;
            self.current_block.content.write_u64(self.len);
            self.current_block.content.pos = old_pos;
            self.current_block.dirty = true;
        } else {
            let (_, buffer) =
                read_block(tx, snapshot, self.branch.keys().read(), &self.head_locator).await?;

            let mut cursor = Cursor::new(buffer);
            cursor.pos = 0;
            cursor.write_u64(self.len);

            write_block(
                tx,
                snapshot,
                read_key,
                write_keys,
                &self.head_locator,
                cursor.buffer,
            )
            .await?;
        }

        self.len_dirty = false;

        Ok(())
    }

    // Write the current block into the store.
    async fn write_current_block(
        &mut self,
        tx: &mut db::WriteTransaction,
        snapshot: &mut SnapshotData,
    ) -> Result<()> {
        if !self.current_block.dirty {
            return Ok(());
        }

        let read_key = self.branch.keys().read();
        let write_keys = self.branch.keys().write().ok_or(Error::PermissionDenied)?;

        let block_id = write_block(
            tx,
            snapshot,
            read_key,
            write_keys,
            &self.current_block.locator,
            self.current_block.content.buffer.clone(),
        )
        .await?;

        self.current_block.id = block_id;
        self.current_block.dirty = false;

        Ok(())
    }
}

/// Creates a shallow copy (only the index nodes are copied, not blocks) of the specified blob into
/// the specified destination branch.
///
/// NOTE: This function is not atomic. However, it is idempotent, so in case it's interrupted, it
/// can be safely retried.
pub(crate) async fn fork(blob_id: BlobId, src_branch: &Branch, dst_branch: &Branch) -> Result<()> {
    // If the blob is already forked, do nothing but still return Ok to maintain idempotency.
    if src_branch.id() == dst_branch.id() {
        return Ok(());
    }

    let read_key = src_branch.keys().read();
    // Take the write key from the dst branch, not the src branch, to protect us against
    // accidentally forking into remote branch (remote branches don't have write access).
    let write_keys = dst_branch.keys().write().ok_or(Error::PermissionDenied)?;

    // To avoid write-blocking the database for too long, we process the blocks in batches. This is
    // the number of blocks per batch.
    // TODO: It is currently set to 1 because the previous value (32) was found to have negative
    // impact on network speed. The Client had to wait a long time to acquire a write transaction
    // and that caused the incoming response messages to queue up.
    const BATCH_SIZE: u32 = 1;

    let end = load_block_count_hint(src_branch, blob_id).await?;
    let mut locators = Locator::head(blob_id).sequence().take(end as usize);
    let mut running = true;

    while running {
        let mut tx = src_branch.db().begin_write().await?;

        let src_snapshot = src_branch.data().load_snapshot(&mut tx).await?;
        let mut dst_snapshot = dst_branch
            .data()
            .load_or_create_snapshot(&mut tx, write_keys)
            .await?;

        for _ in 0..BATCH_SIZE {
            let Some(locator) = locators.next() else {
                running = false;
                break;
            };

            let encoded_locator = locator.encode(read_key);

            let (block_id, block_presence) =
                match src_snapshot.get_block(&mut tx, &encoded_locator).await {
                    Ok(block) => block,
                    Err(Error::EntryNotFound) => {
                        // end of the blob
                        running = false;
                        break;
                    }
                    Err(error) => return Err(error),
                };

            // It can happen that the current and dst branches are different, but the blob has
            // already been forked by some other task in the meantime. In that case this
            // `insert` is a no-op. We still proceed normally to maintain idempotency.
            dst_snapshot
                .insert_block(
                    &mut tx,
                    &encoded_locator,
                    &block_id,
                    block_presence,
                    write_keys,
                )
                .instrument(tracing::info_span!(
                    "fork_block",
                    num = locator.number(),
                    id = ?block_id
                ))
                .await?;
        }

        dst_snapshot.remove_all_older(&mut tx).await?;

        tx.commit().await?;
    }

    Ok(())
}

fn block_count(len: u64) -> u32 {
    // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
    (1 + (len + HEADER_SIZE as u64 - 1) / BLOCK_SIZE as u64)
        .try_into()
        .unwrap_or(u32::MAX)
}

async fn read_block(
    tx: &mut db::ReadTransaction,
    snapshot: &SnapshotData,
    read_key: &cipher::SecretKey,
    locator: &Locator,
) -> Result<(BlockId, Buffer)> {
    let (id, _) = snapshot.get_block(tx, &locator.encode(read_key)).await?;

    let mut buffer = Buffer::new();
    let nonce = block::read(tx, &id, &mut buffer).await?;

    decrypt_block(read_key, &nonce, &mut buffer);

    Ok((id, buffer))
}

#[instrument(skip(tx, snapshot, read_key, write_keys, buffer), fields(id))]
async fn write_block(
    tx: &mut db::WriteTransaction,
    snapshot: &mut SnapshotData,
    read_key: &cipher::SecretKey,
    write_keys: &sign::Keypair,
    locator: &Locator,
    mut buffer: Buffer,
) -> Result<BlockId> {
    let nonce = rand::random();
    encrypt_block(read_key, &nonce, &mut buffer);
    let id = BlockId::from_content(&buffer);

    Span::current().record("id", field::debug(&id));

    // NOTE: make sure the index and block store operations run in the same order as in
    // `load_block` to prevent potential deadlocks when `load_block` and `write_block` run
    // concurrently and `load_block` runs inside a transaction.
    let inserted = snapshot
        .insert_block(
            tx,
            &locator.encode(read_key),
            &id,
            SingleBlockPresence::Present,
            write_keys,
        )
        .await?;
    snapshot.remove_all_older(tx).await?;

    // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
    // hit one in 2^sizeof(BlockId) chance that we randomly generated the same BlockId twice.
    assert!(inserted);

    block::write(tx, &id, &buffer, &nonce).await?;

    Ok(id)
}

async fn read_len(
    tx: &mut db::ReadTransaction,
    snapshot: &SnapshotData,
    read_key: &cipher::SecretKey,
    blob_id: BlobId,
) -> Result<u64> {
    let (_, buffer) = read_block(tx, snapshot, read_key, &Locator::head(blob_id)).await?;
    Ok(Cursor::new(buffer).read_u64())
}

// Returns the max number of blocks the specified blob has. This either returns the actual number
// or `u32::MAX` in case the first blob is not available and so the blob length can't be obtained.
async fn load_block_count_hint(branch: &Branch, blob_id: BlobId) -> Result<u32> {
    let mut tx = branch.db().begin_read().await?;
    let snapshot = branch.data().load_snapshot(&mut tx).await?;

    match read_len(&mut tx, &snapshot, branch.keys().read(), blob_id).await {
        Ok(len) => Ok(block_count(len)),
        Err(Error::BlockNotFound(_)) => Ok(u32::MAX),
        Err(error) => Err(error),
    }
}

fn decrypt_block(blob_key: &cipher::SecretKey, block_nonce: &BlockNonce, content: &mut [u8]) {
    let block_key = SecretKey::derive_from_key(blob_key.as_array(), block_nonce);
    block_key.decrypt_no_aead(&Nonce::default(), content);
}

fn encrypt_block(blob_key: &cipher::SecretKey, block_nonce: &BlockNonce, content: &mut [u8]) {
    let block_key = SecretKey::derive_from_key(blob_key.as_array(), block_nonce);
    block_key.encrypt_no_aead(&Nonce::default(), content);
}

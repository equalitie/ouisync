use super::{
    inner::Unique,
    open_block::{Buffer, Cursor, OpenBlock},
    Shared, HEADER_SIZE,
};
use crate::{
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
    sync::MutexGuard,
};
use sqlx::Connection;
use std::{convert::TryInto, io::SeekFrom};

pub(crate) struct Operations<'a> {
    pub(super) shared: MutexGuard<'a, Shared>,
    pub(super) unique: &'a mut Unique,
}

impl<'a> Operations<'a> {
    /// Was this blob modified and not flushed yet?
    pub fn is_dirty(&self) -> bool {
        self.unique.current_block.dirty || self.unique.len_dirty
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
            let remaining = (self.shared.len - self.seek_position())
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
            if locator.number() >= self.shared.block_count() {
                break;
            }

            let (id, content) = read_block(
                conn,
                self.unique.branch.data(),
                self.unique.branch.keys().read(),
                &locator,
            )
            .await?;

            // NOTE: unlike in `write` we create a separate transaction for each iteration. This is
            // because if we created a single transaction for the whole `read` call, then a failed
            // read could rollback the changes made in a previous iteration which would then be
            // lost. This is fine because there is going to be at most one dirty block within
            // a single `read` invocation anyway.
            let mut tx = conn.begin().await?;
            self.replace_current_block(&mut tx, locator, id, content)
                .await?;
            tx.commit().await?;
        }

        Ok(total_len)
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    pub async fn read_to_end(&mut self, conn: &mut db::Connection) -> Result<Vec<u8>> {
        let mut buffer = vec![
            0;
            (self.shared.len - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX)
        ];

        let len = self.read(conn, &mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    /// Writes into the blob in db transaction.
    pub async fn write(&mut self, tx: &mut db::Transaction<'_>, mut buffer: &[u8]) -> Result<()> {
        loop {
            let len = self.unique.current_block.content.write(buffer);

            // TODO: only set the dirty flag if the content actually changed. Otherwise overwirting
            // a block with the same content it already had would result in a new block with a new
            // version being unnecessarily created.
            if len > 0 {
                self.unique.current_block.dirty = true;
            }

            buffer = &buffer[len..];

            if self.seek_position() > self.shared.len {
                self.shared.len = self.seek_position();
                self.unique.len_dirty = true;
            }

            if buffer.is_empty() {
                break;
            }

            let locator = self.unique.current_block.locator.next();
            let (id, content) = if locator.number() < self.shared.block_count() {
                read_block(
                    tx,
                    self.unique.branch.data(),
                    self.unique.branch.keys().read(),
                    &locator,
                )
                .await?
            } else {
                (BlockId::from_content(&[]), Buffer::new())
            };

            self.replace_current_block(tx, locator, id, content).await?;
        }

        Ok(())
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub async fn seek(&mut self, tx: &mut db::Transaction<'_>, pos: SeekFrom) -> Result<u64> {
        let offset = match pos {
            SeekFrom::Start(n) => n.min(self.shared.len),
            SeekFrom::End(n) => {
                if n >= 0 {
                    self.shared.len
                } else {
                    self.shared.len.saturating_sub((-n) as u64)
                }
            }
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.seek_position()
                        .saturating_add(n as u64)
                        .min(self.shared.len)
                } else {
                    self.seek_position().saturating_sub((-n) as u64)
                }
            }
        };

        let actual_offset = offset + HEADER_SIZE as u64;
        let block_number = (actual_offset / BLOCK_SIZE as u64) as u32;
        let block_offset = (actual_offset % BLOCK_SIZE as u64) as usize;

        if block_number != self.unique.current_block.locator.number() {
            let locator = self.locator_at(block_number);

            let (id, content) = read_block(
                tx,
                self.unique.branch.data(),
                self.unique.branch.keys().read(),
                &locator,
            )
            .await?;
            self.replace_current_block(tx, locator, id, content).await?;
        }

        self.unique.current_block.content.pos = block_offset;

        Ok(offset)
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, tx: &mut db::Transaction<'_>, len: u64) -> Result<()> {
        if len == self.shared.len {
            return Ok(());
        }

        if len > self.shared.len {
            // TODO: consider supporting this
            return Err(Error::OperationNotSupported);
        }

        let old_block_count = self.shared.block_count();

        self.shared.len = len;
        self.unique.len_dirty = true;

        let new_block_count = self.shared.block_count();

        if self.seek_position() > self.shared.len {
            self.seek(tx, SeekFrom::End(0)).await?;
        }

        remove_blocks(
            tx,
            &self.unique.branch,
            self.unique
                .head_locator
                .sequence()
                .skip(new_block_count as usize)
                .take((old_block_count - new_block_count) as usize),
        )
        .await
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    /// Return true if was dirty and the flush actually took place
    pub async fn flush(&mut self, tx: &mut db::Transaction<'_>) -> Result<bool> {
        if !self.is_dirty() {
            return Ok(false);
        }

        self.write_len(tx).await?;
        self.write_current_block(tx).await?;

        Ok(true)
    }

    async fn replace_current_block(
        &mut self,
        tx: &mut db::Transaction<'_>,
        locator: Locator,
        id: BlockId,
        content: Buffer,
    ) -> Result<()> {
        self.write_current_block(tx).await?;

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
            self.unique.current_block.content.write_u64(self.shared.len);
            self.unique.current_block.content.pos = old_pos;
            self.unique.current_block.dirty = true;
        } else {
            let locator = self.locator_at(0);
            let (_, buffer) = read_block(
                tx,
                self.unique.branch.data(),
                self.unique.branch.keys().read(),
                &locator,
            )
            .await?;

            let mut cursor = Cursor::new(buffer);
            cursor.pos = 0;
            cursor.write_u64(self.shared.len);

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

    // Returns the current seek position from the start of the blob.
    pub fn seek_position(&mut self) -> u64 {
        self.unique.current_block.locator.number() as u64 * BLOCK_SIZE as u64
            + self.unique.current_block.content.pos as u64
            - HEADER_SIZE as u64
    }

    fn locator_at(&self, number: u32) -> Locator {
        self.unique.head_locator.nth(number)
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

pub(super) async fn load_block(
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
    branch
        .insert(tx, &id, &locator.encode(read_key), write_keys)
        .await?;
    block::write(tx, &id, &buffer, &nonce).await?;

    // Pin locally created blocks so they are not prematurelly collected. All pinned blocks are
    // unpinned when the current changeset is flushed at which point the block should already be
    // reachable. The pins are not persistent so if the app is terminated or crashes before the
    // current changeset is flushed, all unreachable blocks will be subject to collection on the
    // next app start.
    block::pin(tx, &id).await?;

    Ok(id)
}

pub(super) fn decrypt_block(
    blob_key: &cipher::SecretKey,
    block_nonce: &BlockNonce,
    content: &mut [u8],
) {
    let block_key = SecretKey::derive_from_key(blob_key.as_ref(), block_nonce);
    block_key.decrypt_no_aead(&Nonce::default(), content);
}

pub(super) fn encrypt_block(
    blob_key: &cipher::SecretKey,
    block_nonce: &BlockNonce,
    content: &mut [u8],
) {
    let block_key = SecretKey::derive_from_key(blob_key.as_ref(), block_nonce);
    block_key.encrypt_no_aead(&Nonce::default(), content);
}

pub(super) async fn remove_blocks<T>(
    tx: &mut db::Transaction<'_>,
    branch: &Branch,
    locators: T,
) -> Result<()>
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

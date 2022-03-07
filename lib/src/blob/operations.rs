use super::{Blob, Buffer, Core, Cursor, Inner, OpenBlock, HEADER_SIZE};
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
};
use sqlx::Connection;
use std::{convert::TryInto, io::SeekFrom, sync::Arc};
use tokio::sync::{Mutex, MutexGuard};

pub(crate) struct Operations<'a> {
    pub(super) core: MutexGuard<'a, Core>,
    pub(super) branch: &'a Branch,
    pub(super) head_locator: &'a Locator,
    pub(super) current_block: &'a mut OpenBlock,
}

impl<'a> Operations<'a> {
    /// Was this blob modified and not flushed yet?
    pub fn is_dirty(&self) -> bool {
        self.current_block.dirty || self.core.len_dirty
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
            let remaining = (self.core.len - self.seek_position())
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
            if locator.number() >= self.core.block_count() {
                break;
            }

            let (id, content) = read_block(
                conn,
                self.branch.data(),
                self.branch.keys().read(),
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
            (self.core.len - self.seek_position())
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
            let len = self.current_block.content.write(buffer);

            // TODO: only set the dirty flag if the content actually changed. Otherwise overwirting
            // a block with the same content it already had would result in a new block with a new
            // version being unnecessarily created.
            if len > 0 {
                self.current_block.dirty = true;
            }

            buffer = &buffer[len..];

            if self.seek_position() > self.core.len {
                self.core.len = self.seek_position();
                self.core.len_dirty = true;
            }

            if buffer.is_empty() {
                break;
            }

            let locator = self.current_block.locator.next();
            let (id, content) = if locator.number() < self.core.block_count() {
                read_block(tx, self.branch.data(), self.branch.keys().read(), &locator).await?
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
            SeekFrom::Start(n) => n.min(self.core.len),
            SeekFrom::End(n) => {
                if n >= 0 {
                    self.core.len
                } else {
                    self.core.len.saturating_sub((-n) as u64)
                }
            }
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.seek_position()
                        .saturating_add(n as u64)
                        .min(self.core.len)
                } else {
                    self.seek_position().saturating_sub((-n) as u64)
                }
            }
        };

        let actual_offset = offset + HEADER_SIZE as u64;
        let block_number = (actual_offset / BLOCK_SIZE as u64) as u32;
        let block_offset = (actual_offset % BLOCK_SIZE as u64) as usize;

        if block_number != self.current_block.locator.number() {
            let locator = self.locator_at(block_number);

            let (id, content) =
                read_block(tx, self.branch.data(), self.branch.keys().read(), &locator).await?;
            self.replace_current_block(tx, locator, id, content).await?;
        }

        self.current_block.content.pos = block_offset;

        Ok(offset)
    }

    /// Truncate the blob to the given length.
    pub async fn truncate(&mut self, tx: &mut db::Transaction<'_>, len: u64) -> Result<()> {
        // TODO: reuse the truncated blocks on subsequent writes if the content is identical

        if len == self.core.len {
            return Ok(());
        }

        if len > self.core.len {
            // TODO: consider supporting this
            return Err(Error::OperationNotSupported);
        }

        let old_block_count = self.core.block_count();

        self.core.len = len;
        self.core.len_dirty = true;

        let new_block_count = self.core.block_count();

        if self.seek_position() > self.core.len {
            self.seek(tx, SeekFrom::End(0)).await?;
        }

        self.remove_blocks(
            tx,
            self.head_locator
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

    /// Removes this blob.
    pub async fn remove(&mut self, conn: &mut db::Connection) -> Result<()> {
        let mut tx = conn.begin().await?;
        self.remove_blocks(
            &mut tx,
            self.head_locator
                .sequence()
                .take(self.core.block_count() as usize),
        )
        .await?;
        tx.commit().await?;

        *self.current_block = OpenBlock::new_head(*self.head_locator);
        self.core.len = 0;
        self.core.len_dirty = true;

        Ok(())
    }

    /// Creates a shallow copy (only the index nodes are copied, not blocks) of this blob into the
    /// specified destination branch and locator.
    pub async fn fork(
        &mut self,
        conn: &mut db::Connection,
        dst_branch: Branch,
        dst_head_locator: Locator,
    ) -> Result<Blob> {
        // This should gracefuly handled in the Blob from where this function is invoked.
        assert!(self.branch.id() != dst_branch.id() || *self.head_locator != dst_head_locator);

        let read_key = self.branch.keys().read();
        // Take the write key from the dst branch, not the src branch, to protect us against
        // accidentally forking into remote branch (remote branches don't have write access).
        let write_keys = dst_branch.keys().write().ok_or(Error::PermissionDenied)?;

        let mut tx = conn.begin().await?;
        let mut dst_writer = dst_branch.data().write();

        for (src_locator, dst_locator) in self.locators().zip(dst_head_locator.sequence()) {
            let encoded_src_locator = src_locator.encode(read_key);
            let encoded_dst_locator = dst_locator.encode(read_key);

            let block_id = self
                .branch
                .data()
                .get(&mut tx, &encoded_src_locator)
                .await?;

            dst_writer
                .insert(&mut tx, &block_id, &encoded_dst_locator, write_keys)
                .await?;
        }

        dst_writer.finish().await;
        tx.commit().await?;

        let new_core = Core {
            len: self.core.len,
            len_dirty: self.core.len_dirty,
        };

        let current_block = OpenBlock {
            locator: dst_head_locator.nth(self.current_block.locator.number()),
            id: self.current_block.id,
            content: self.current_block.content.clone(),
            dirty: self.current_block.dirty,
        };

        Ok(Blob {
            core: Arc::new(Mutex::new(new_core)),
            inner: Inner {
                branch: dst_branch,
                head_locator: dst_head_locator,
                current_block,
            },
        })
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
            content.pos = HEADER_SIZE;
        }

        *self.current_block = OpenBlock {
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

        let read_key = self.branch.keys().read();
        let write_keys = self.branch.keys().write().ok_or(Error::PermissionDenied)?;

        let block_id = write_block(
            tx,
            self.branch.data(),
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

    // Write the current blob length into the blob header in the head block.
    async fn write_len(&mut self, tx: &mut db::Transaction<'_>) -> Result<()> {
        if !self.core.len_dirty {
            return Ok(());
        }

        let read_key = self.branch.keys().read();
        let write_keys = self.branch.keys().write().ok_or(Error::PermissionDenied)?;

        if self.current_block.locator.number() == 0 {
            let old_pos = self.current_block.content.pos;
            self.current_block.content.pos = 0;
            self.current_block.content.write_u64(self.core.len);
            self.current_block.content.pos = old_pos;
            self.current_block.dirty = true;
        } else {
            let locator = self.locator_at(0);
            let (_, buffer) =
                read_block(tx, self.branch.data(), self.branch.keys().read(), &locator).await?;

            let mut cursor = Cursor::new(buffer);
            cursor.pos = 0;
            cursor.write_u64(self.core.len);

            write_block(
                tx,
                self.branch.data(),
                read_key,
                write_keys,
                &locator,
                cursor.buffer,
            )
            .await?;
        }

        self.core.len_dirty = false;

        Ok(())
    }

    async fn remove_blocks<T>(&self, tx: &mut db::Transaction<'_>, locators: T) -> Result<()>
    where
        T: IntoIterator<Item = Locator>,
    {
        let read_key = self.branch.keys().read();
        let write_keys = self.branch.keys().write().ok_or(Error::PermissionDenied)?;

        let mut writer = self.branch.data().write();

        for locator in locators {
            writer
                .remove(tx, &locator.encode(read_key), write_keys)
                .await?;
        }

        writer.finish().await;

        Ok(())
    }

    // Returns the current seek position from the start of the blob.
    pub fn seek_position(&mut self) -> u64 {
        self.current_block.locator.number() as u64 * BLOCK_SIZE as u64
            + self.current_block.content.pos as u64
            - HEADER_SIZE as u64
    }

    fn locator_at(&self, number: u32) -> Locator {
        self.head_locator.nth(number)
    }

    fn locators(&self) -> impl Iterator<Item = Locator> {
        self.head_locator
            .sequence()
            .take(self.core.block_count() as usize)
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

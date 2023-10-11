pub(crate) mod lock;

mod block_ids;
mod id;
mod position;

#[cfg(test)]
mod tests;

pub(crate) use self::{block_ids::BlockIds, id::BlobId};

use self::position::Position;
use crate::{
    branch::Branch,
    collections::{hash_map::Entry, HashMap},
    crypto::{
        cipher::{self, Nonce, SecretKey},
        sign::{Keypair, PublicKey},
    },
    error::{Error, Result},
    protocol::{
        Block, BlockContent, BlockId, BlockNonce, Locator, RootNode, SingleBlockPresence,
        BLOCK_SIZE,
    },
    store::{self, Changeset, ReadTransaction},
};
use std::{io::SeekFrom, iter, mem};
use thiserror::Error;

/// Size of the blob header in bytes.
// Using u64 instead of usize because HEADER_SIZE must be the same irrespective of whether we're on
// a 32bit or 64bit processor (if we want two such replicas to be able to sync).
pub const HEADER_SIZE: usize = mem::size_of::<u64>();

// Max number of blocks in the cache. Increasing this number decreases the number of flushes needed
// during writes but increases the coplexity of the individual flushes.
const CACHE_CAPACITY: usize = 2048; // 64 MiB

#[derive(Debug, Error)]
pub(crate) enum ReadWriteError {
    #[error("block not found in the cache")]
    CacheMiss,
    #[error("cache is full")]
    CacheFull,
}

pub(crate) struct Blob {
    branch: Branch,
    id: BlobId,
    cache: HashMap<u32, CachedBlock>,
    len_original: u64,
    len_modified: u64,
    position: Position,
}

impl Blob {
    /// Opens an existing blob.
    pub async fn open(tx: &mut ReadTransaction, branch: Branch, id: BlobId) -> Result<Self> {
        let root_node = tx.load_root_node(branch.id()).await?;
        Self::open_at(tx, &root_node, branch, id).await
    }

    pub async fn open_at(
        tx: &mut ReadTransaction,
        root_node: &RootNode,
        branch: Branch,
        id: BlobId,
    ) -> Result<Self> {
        assert_eq!(root_node.proof.writer_id, *branch.id());

        let (_, buffer) =
            read_block(tx, root_node, &Locator::head(id), branch.keys().read()).await?;

        let len = buffer.read_u64(0);
        let cached_block = CachedBlock::from(buffer);
        let cache = iter::once((0, cached_block)).collect();
        let position = Position::ZERO;

        Ok(Self {
            branch,
            id,
            cache,
            len_original: len,
            len_modified: len,
            position,
        })
    }

    /// Creates a new blob.
    pub fn create(branch: Branch, id: BlobId) -> Self {
        let cached_block = CachedBlock::new().with_dirty(true);
        let cache = iter::once((0, cached_block)).collect();

        Self {
            branch,
            id,
            cache,
            len_original: 0,
            len_modified: 0,
            position: Position::ZERO,
        }
    }

    pub fn branch(&self) -> &Branch {
        &self.branch
    }

    /// Id of this blob.
    pub fn id(&self) -> &BlobId {
        &self.id
    }

    /// Length of this blob in bytes.
    pub fn len(&self) -> u64 {
        self.len_modified
    }

    // Returns the current seek position from the start of the blob.
    pub fn seek_position(&self) -> u64 {
        self.position.get()
    }

    pub fn block_count(&self) -> u32 {
        block_count(self.len())
    }

    /// Was this blob modified and not flushed yet?
    pub fn is_dirty(&self) -> bool {
        self.cache.values().any(|block| block.dirty) || self.len_modified != self.len_original
    }

    /// Seek to an offset in the blob.
    ///
    /// It is allowed to specify offset that is outside of the range of the blob but such offset
    /// will be clamped to be within the range.
    ///
    /// Returns the new seek position from the start of the blob.
    pub fn seek(&mut self, pos: SeekFrom) -> u64 {
        let position = match pos {
            SeekFrom::Start(n) => n.min(self.len()),
            SeekFrom::End(n) => {
                if n >= 0 {
                    self.len()
                } else {
                    self.len().saturating_sub((-n) as u64)
                }
            }
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.seek_position()
                        .saturating_add(n as u64)
                        .min(self.len())
                } else {
                    self.seek_position().saturating_sub((-n) as u64)
                }
            }
        };

        self.position.set(position);

        position
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()`.
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize, ReadWriteError> {
        if self.position.get() >= self.len() {
            return Ok(0);
        }

        let block = match self.cache.get(&self.position.block) {
            Some(block) => block,
            None => {
                if self.check_cache_capacity() {
                    return Err(ReadWriteError::CacheMiss);
                } else {
                    return Err(ReadWriteError::CacheFull);
                }
            }
        };

        // minimum of:
        // - buffer length
        // - remaining size of the current block
        // - remaining size of the whole blob
        let read_len = buffer
            .len()
            .min(block.content.len() - self.position.offset)
            .min(self.len() as usize - self.position.get() as usize);

        block
            .content
            .read(self.position.offset, &mut buffer[..read_len]);

        self.position.advance(read_len);

        Ok(read_len)
    }

    #[cfg(test)]
    pub async fn read_all(&mut self, tx: &mut ReadTransaction, buffer: &mut [u8]) -> Result<usize> {
        let root_node = tx.load_root_node(self.branch.id()).await?;
        self.read_all_at(tx, &root_node, buffer).await
    }

    pub async fn read_all_at(
        &mut self,
        tx: &mut ReadTransaction,
        root_node: &RootNode,
        buffer: &mut [u8],
    ) -> Result<usize> {
        assert_eq!(root_node.proof.writer_id, *self.branch.id());

        let mut offset = 0;

        loop {
            match self.read(&mut buffer[offset..]) {
                Ok(0) => break,
                Ok(len) => {
                    offset += len;
                }
                Err(ReadWriteError::CacheMiss) => self.warmup_at(tx, root_node).await?,
                Err(ReadWriteError::CacheFull) => {
                    tracing::error!("cache full");
                    return Err(Error::OperationNotSupported);
                }
            }
        }

        Ok(offset)
    }

    /// Read all data from this blob from the current seek position until the end and return then
    /// in a `Vec`.
    #[cfg(test)]
    pub async fn read_to_end(&mut self, tx: &mut ReadTransaction) -> Result<Vec<u8>> {
        let root_node = tx.load_root_node(self.branch.id()).await?;
        self.read_to_end_at(tx, &root_node).await
    }

    /// Read all data from this blob at the given snapshot from the current seek position until the
    /// end and return then in a `Vec`.
    pub async fn read_to_end_at(
        &mut self,
        tx: &mut ReadTransaction,
        root_node: &RootNode,
    ) -> Result<Vec<u8>> {
        let mut buffer = vec![
            0;
            (self.len() - self.seek_position())
                .try_into()
                .unwrap_or(usize::MAX)
        ];

        self.read_all_at(tx, root_node, &mut buffer).await?;

        Ok(buffer)
    }

    pub fn write(&mut self, buffer: &[u8]) -> Result<usize, ReadWriteError> {
        if buffer.is_empty() {
            return Ok(0);
        }

        let block = match self.cache.get_mut(&self.position.block) {
            Some(block) => block,
            None => {
                if !self.check_cache_capacity() {
                    return Err(ReadWriteError::CacheFull);
                }

                if self.position.get() >= self.len_modified
                    || self.position.offset == 0 && buffer.len() >= BLOCK_SIZE
                {
                    self.cache.entry(self.position.block).or_default()
                } else {
                    return Err(ReadWriteError::CacheMiss);
                }
            }
        };

        let write_len = buffer.len().min(block.content.len() - self.position.offset);

        block
            .content
            .write(self.position.offset, &buffer[..write_len]);
        block.dirty = true;

        self.position.advance(write_len);
        self.len_modified = self.len_modified.max(self.position.get());

        Ok(write_len)
    }

    pub async fn write_all(
        &mut self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
        buffer: &[u8],
    ) -> Result<()> {
        let mut offset = 0;

        loop {
            match self.write(&buffer[offset..]) {
                Ok(0) => break,
                Ok(len) => {
                    offset += len;
                }
                Err(ReadWriteError::CacheMiss) => {
                    self.warmup(tx).await?;
                }
                Err(ReadWriteError::CacheFull) => {
                    self.flush(tx, changeset).await?;
                }
            }
        }

        Ok(())
    }

    /// Load the current block into the cache.
    pub async fn warmup(&mut self, tx: &mut ReadTransaction) -> Result<()> {
        let root_node = tx.load_root_node(self.branch.id()).await?;
        self.warmup_at(tx, &root_node).await?;

        Ok(())
    }

    /// Load the current block at the given snapshot into the cache.
    pub async fn warmup_at(
        &mut self,
        tx: &mut ReadTransaction,
        root_node: &RootNode,
    ) -> Result<()> {
        match self.cache.entry(self.position.block) {
            Entry::Occupied(_) => (),
            Entry::Vacant(entry) => {
                let locator = Locator::head(self.id).nth(self.position.block);
                let (_, buffer) =
                    read_block(tx, root_node, &locator, self.branch.keys().read()).await?;
                entry.insert(CachedBlock::from(buffer));
            }
        }

        Ok(())
    }

    /// Truncate the blob to the given length.
    pub fn truncate(&mut self, len: u64) -> Result<()> {
        if len == self.len() {
            return Ok(());
        }

        if len > self.len() {
            // TODO: consider supporting this
            return Err(Error::OperationNotSupported);
        }

        if self.seek_position() > len {
            self.seek(SeekFrom::Start(len));
        }

        self.len_modified = len;

        Ok(())
    }

    /// Flushes this blob, ensuring that all intermediately buffered contents gets written to the
    /// store.
    pub(crate) async fn flush(
        &mut self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
    ) -> Result<()> {
        self.write_len(tx, changeset).await?;
        self.write_blocks(changeset);

        Ok(())
    }

    fn check_cache_capacity(&mut self) -> bool {
        if self.cache.len() < CACHE_CAPACITY {
            return true;
        }

        let number = self
            .cache
            .iter()
            .find(|(_, block)| !block.dirty)
            .map(|(number, _)| *number);

        if let Some(number) = number {
            self.cache.remove(&number);
            true
        } else {
            false
        }
    }

    // Write length, if changed
    async fn write_len(
        &mut self,
        tx: &mut ReadTransaction,
        changeset: &mut Changeset,
    ) -> Result<()> {
        if self.len_modified == self.len_original {
            return Ok(());
        }

        if let Some(block) = self.cache.get_mut(&0) {
            block.content.write_u64(0, self.len_modified);
            block.dirty = true;
        } else {
            let locator = Locator::head(self.id);
            let root_node = tx.load_root_node(self.branch.id()).await?;
            let (_, mut content) =
                read_block(tx, &root_node, &locator, self.branch.keys().read()).await?;
            content.write_u64(0, self.len_modified);
            write_block(changeset, &locator, content, self.branch.keys().read());
        }

        self.len_original = self.len_modified;

        Ok(())
    }

    fn write_blocks(&mut self, changeset: &mut Changeset) {
        // Poor man's `drain_filter`.
        let cache = mem::take(&mut self.cache);
        let (dirty, clean): (HashMap<_, _>, _) =
            cache.into_iter().partition(|(_, block)| block.dirty);
        self.cache = clean;

        for (number, block) in dirty {
            let locator = Locator::head(self.id).nth(number);
            write_block(
                changeset,
                &locator,
                block.content,
                self.branch.keys().read(),
            );
        }
    }
}

// NOTE: Clone only creates a new instance of the same blob. It doesn't preserve dirtiness.
impl Clone for Blob {
    fn clone(&self) -> Self {
        Self {
            branch: self.branch.clone(),
            id: self.id,
            cache: HashMap::default(),
            len_original: self.len_original,
            len_modified: self.len_original,
            position: self.position,
        }
    }
}

#[derive(Default)]
struct CachedBlock {
    content: BlockContent,
    dirty: bool,
}

impl CachedBlock {
    fn new() -> Self {
        Self::default()
    }

    fn with_dirty(self, dirty: bool) -> Self {
        Self { dirty, ..self }
    }
}

impl From<BlockContent> for CachedBlock {
    fn from(content: BlockContent) -> Self {
        Self {
            content,
            dirty: false,
        }
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

    // FIXME: The src blob can change in the middle of the fork which could cause the dst blob to
    // become corrupted (part of it will be forked pre-change and part post-change). To prevent
    // that, we should restart the fork every time the src branch changes, or - better - run the
    // whole fork in a single transaction (but somehow avoid blocking other tasks).

    let end = {
        let mut tx = src_branch.store().begin_read().await?;
        let root_node = tx.load_root_node(src_branch.id()).await?;
        load_block_count_hint(&mut tx, &root_node, blob_id, src_branch.keys().read()).await?
    };

    struct Batch {
        tx: crate::store::WriteTransaction,
        changeset: Changeset,
    }

    impl Batch {
        async fn apply(self, dst_branch_id: &PublicKey, write_keys: &Keypair) -> Result<()> {
            let Batch { mut tx, changeset } = self;
            changeset.apply(&mut tx, dst_branch_id, write_keys).await?;
            tx.commit().await?;
            Ok(())
        }
    }

    // Based on some benchmarking it seems that batch values don't hurt the syncing performance too
    // much. https://github.com/equalitie/ouisync/issues/143#issuecomment-1757951167
    let batch_size = 2048;
    let mut opt_batch = None;

    let locators = Locator::head(blob_id).sequence().take(end as usize);

    for locator in locators {
        if opt_batch.is_none() {
            let tx = src_branch.store().begin_write().await?;
            let changeset = Changeset::new();
            opt_batch = Some(Batch { tx, changeset });
        }

        let Batch { tx, changeset } = opt_batch.as_mut().unwrap();

        let encoded_locator = locator.encode(read_key);

        let block_id = match tx.find_block(src_branch.id(), &encoded_locator).await {
            Ok(id) => id,
            Err(store::Error::LocatorNotFound) => {
                // end of the blob
                break;
            }
            Err(error) => return Err(error.into()),
        };

        let block_presence = if tx.block_exists(&block_id).await? {
            SingleBlockPresence::Present
        } else {
            SingleBlockPresence::Missing
        };

        changeset.link_block(encoded_locator, block_id, block_presence);

        // The `+ 1` is there to not hit on the first run.
        if (locator.number() + 1) % batch_size == 0 {
            opt_batch
                .take()
                .unwrap()
                .apply(dst_branch.id(), write_keys)
                .await?;
        }

        tracing::trace!(
            num = locator.number(),
            block_id = ?block_id,
            ?block_presence,
            "fork block",
        );
    }

    if let Some(batch) = opt_batch {
        batch.apply(dst_branch.id(), write_keys).await?;
    }

    Ok(())
}

fn block_count(len: u64) -> u32 {
    // https://stackoverflow.com/questions/2745074/fast-ceiling-of-an-integer-division-in-c-c
    (1 + (len + HEADER_SIZE as u64 - 1) / BLOCK_SIZE as u64)
        .try_into()
        .unwrap_or(u32::MAX)
}

async fn read_len(
    tx: &mut ReadTransaction,
    root_node: &RootNode,
    blob_id: BlobId,
    read_key: &cipher::SecretKey,
) -> Result<u64> {
    let (_, buffer) = read_block(tx, root_node, &Locator::head(blob_id), read_key).await?;
    Ok(buffer.read_u64(0))
}

// Returns the max number of blocks the specified blob has. This either returns the actual number
// or `u32::MAX` in case the first blob is not available and so the blob length can't be obtained.
async fn load_block_count_hint(
    tx: &mut ReadTransaction,
    root_node: &RootNode,
    blob_id: BlobId,
    read_key: &cipher::SecretKey,
) -> Result<u32> {
    match read_len(tx, root_node, blob_id, read_key).await {
        Ok(len) => Ok(block_count(len)),
        Err(Error::Store(store::Error::BlockNotFound)) => Ok(u32::MAX),
        Err(error) => Err(error),
    }
}

async fn read_block(
    tx: &mut ReadTransaction,
    root_node: &RootNode,
    locator: &Locator,
    read_key: &cipher::SecretKey,
) -> Result<(BlockId, BlockContent)> {
    let id = tx
        .find_block_at(root_node, &locator.encode(read_key))
        .await?;

    let mut content = BlockContent::new();
    let nonce = tx.read_block(&id, &mut content).await?;

    decrypt_block(read_key, &nonce, &mut content);

    Ok((id, content))
}

fn write_block(
    changeset: &mut Changeset,
    locator: &Locator,
    mut content: BlockContent,
    read_key: &cipher::SecretKey,
) -> BlockId {
    let nonce = rand::random();
    encrypt_block(read_key, &nonce, &mut content);

    let block = Block::new(content, nonce);
    let block_id = block.id;

    changeset.link_block(
        locator.encode(read_key),
        block.id,
        SingleBlockPresence::Present,
    );
    changeset.write_block(block);

    tracing::trace!(?locator, ?block_id, "write block");

    block_id
}

fn decrypt_block(blob_key: &cipher::SecretKey, block_nonce: &BlockNonce, content: &mut [u8]) {
    let block_key = SecretKey::derive_from_key(blob_key.as_array(), block_nonce);
    block_key.decrypt_no_aead(&Nonce::default(), content);
}

fn encrypt_block(blob_key: &cipher::SecretKey, block_nonce: &BlockNonce, content: &mut [u8]) {
    let block_key = SecretKey::derive_from_key(blob_key.as_array(), block_nonce);
    block_key.encrypt_no_aead(&Nonce::default(), content);
}

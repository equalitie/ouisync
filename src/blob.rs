use crate::{
    block::{self, BlockId, BlockName, BlockVersion, BLOCK_SIZE},
    crypto::{
        aead::{AeadInPlace, NewAead},
        Cipher, NonceSequence, SecretKey, NONCE_PREFIX_SIZE,
    },
    db,
    error::Error,
    index::{self, BlockKind, ChildTag},
};
use std::{
    convert::TryInto,
    ops::{Deref, DerefMut},
};
use zeroize::Zeroize;

pub struct Blob {
    pool: db::Pool,
    secret_key: SecretKey,
    nonce_sequence: NonceSequence,
    current_block: OpenBlock,
}

// TODO: figure out how to implement `flush` on `Drop`.

impl Blob {
    /// Opens an existing blob.
    pub async fn open(
        pool: db::Pool,
        secret_key: SecretKey,
        directory_name: Option<BlockName>,
        directory_seq: u32,
        id: BlockId,
    ) -> Result<Self, Error> {
        // Check directory name matches.
        if let Some(parent_name) = &directory_name {
            let child_tag = ChildTag::new(&secret_key, parent_name, directory_seq, BlockKind::Head);
            match index::get(&pool, &child_tag).await {
                Ok(actual_id) if actual_id == id => (),
                Ok(_) | Err(Error::BlockIdNotFound) => return Err(Error::WrongDirectoryEntry),
                Err(error) => return Err(error),
            }
        }

        let mut content = BlockBuffer::new();
        let auth_tag = block::read(&pool, &id, &mut content).await?;

        // Read nonce prefix
        let position = NONCE_PREFIX_SIZE;
        let nonce_prefix = content[..position].try_into().unwrap();
        let nonce_sequence = NonceSequence::with_prefix(nonce_prefix);

        let nonce = nonce_sequence.get(0);
        let aad = id.to_array(); // "additional associated data"

        let cipher = Cipher::new(secret_key.as_array());
        cipher.decrypt_in_place_detached(&nonce, &aad, &mut content[position..], &auth_tag)?;

        let current_block = OpenBlock {
            parent_name: directory_name,
            seq: directory_seq,
            kind: BlockKind::Head,
            id,
            content,
            position,
        };

        Ok(Self {
            pool,
            secret_key,
            nonce_sequence,
            current_block,
        })
    }

    /// Creates new blob.
    pub async fn create(
        pool: db::Pool,
        secret_key: SecretKey,
        directory_name: Option<BlockName>,
        directory_seq: u32,
        name: BlockName,
    ) -> Result<Self, Error> {
        // Check the directory entry is unique
        if let Some(parent_name) = &directory_name {
            let child_tag = ChildTag::new(&secret_key, parent_name, directory_seq, BlockKind::Head);
            if index::exists(&pool, &child_tag).await? {
                return Err(Error::WrongDirectoryEntry);
            }
        }

        let nonce_sequence = NonceSequence::random();
        let position = nonce_sequence.prefix().len();
        let mut content = BlockBuffer::new();
        content[..position].copy_from_slice(nonce_sequence.prefix());

        let current_block = OpenBlock {
            parent_name: directory_name,
            seq: directory_seq,
            kind: BlockKind::Head,
            id: BlockId {
                name,
                version: BlockVersion::random(),
            },
            content,
            position,
        };

        Ok(Self {
            pool,
            secret_key,
            nonce_sequence,
            current_block,
        })
    }

    /// Reads data from this blob into `buffer`, advancing the internal cursor. Returns the
    /// number of bytes actually read which might be less than `buffer.len()` if the portion of the
    /// blob past the internal cursor is smaller than `buffer.len()`.
    pub async fn read(&mut self, mut buffer: &mut [u8]) -> Result<usize, Error> {
        let mut total_len = 0;

        loop {
            let len = self.current_block.read(buffer);
            buffer = &mut buffer[len..];
            total_len += len;

            if buffer.is_empty() {
                break;
            }

            self.write_current_block().await?;
            if !self.get_next_block().await? {
                break;
            }
        }

        Ok(total_len)
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, mut buffer: &[u8]) -> Result<(), Error> {
        // TODO: do all the db writes in a transaction

        loop {
            let len = self.current_block.write(buffer);
            buffer = &buffer[len..];

            if buffer.is_empty() {
                break;
            }

            self.write_current_block().await?;
            self.get_or_create_next_block().await?;
        }

        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), Error> {
        self.write_current_block().await

        // TODO: write the blob length into the first block
    }

    async fn write_current_block(&self) -> Result<(), Error> {
        // Read the plaintext into the buffer.
        let mut buffer = self.current_block.content.clone();

        let nonce = self.nonce_sequence.get(self.current_block.nonce_index());
        let aad = self.current_block.id.to_array(); // "additional associated data"

        // Encrypt in place.
        let cipher = Cipher::new(self.secret_key.as_array());
        let auth_tag = cipher.encrypt_in_place_detached(&nonce, &aad, &mut buffer)?;

        // Write the block to the block store.
        block::write(&self.pool, &self.current_block.id, &buffer, &auth_tag).await?;

        // Write the block to the index unless it is the head block of the root blob.
        if let Some(parent_name) = &self.current_block.parent_name {
            let child_tag = ChildTag::new(
                &self.secret_key,
                parent_name,
                self.current_block.seq,
                self.current_block.kind,
            );
            index::insert(&self.pool, &self.current_block.id, &child_tag).await?;
        }

        Ok(())
    }

    async fn get_next_block(&mut self) -> Result<bool, Error> {
        let (parent_name, seq) = self.next_block_details();

        if let Some((id, content)) = self.read_next_block(&parent_name, seq).await? {
            self.current_block = OpenBlock::trunk(parent_name, seq, id, content);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn get_or_create_next_block(&mut self) -> Result<(), Error> {
        let (parent_name, seq) = self.next_block_details();

        let (id, content) =
            if let Some(id_and_content) = self.read_next_block(&parent_name, seq).await? {
                // Existing block.
                id_and_content
            } else {
                // New block
                (BlockId::random(), BlockBuffer::new())
            };

        self.current_block = OpenBlock::trunk(parent_name, seq, id, content);

        Ok(())
    }

    fn next_block_details(&self) -> (BlockName, u32) {
        let parent_name = if let Some(name) = self.current_block.parent_name {
            name
        } else {
            // Only the head block of the root blob can have no parent.
            assert_eq!(self.current_block.seq, 0);
            assert_eq!(self.current_block.kind, BlockKind::Head);

            self.current_block.id.name
        };

        let seq = self.current_block.next_seq();

        (parent_name, seq)
    }

    async fn read_next_block(
        &self,
        parent_name: &BlockName,
        seq: u32,
    ) -> Result<Option<(BlockId, BlockBuffer)>, Error> {
        let child_tag = ChildTag::new(&self.secret_key, parent_name, seq, BlockKind::Trunk);

        match index::get(&self.pool, &child_tag).await {
            Ok(id) => {
                let mut content = BlockBuffer::new();
                let auth_tag = block::read(&self.pool, &id, &mut content).await?;

                let nonce = self.nonce_sequence.get(seq);
                let aad = id.to_array(); // "additional associated data"

                let cipher = Cipher::new(self.secret_key.as_array());
                cipher.decrypt_in_place_detached(&nonce, &aad, &mut content, &auth_tag)?;

                Ok(Some((id, content)))
            }
            Err(Error::BlockIdNotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

// Buffer for keeping loaded block content and also for in-place encryption and decryption.
#[derive(Clone)]
struct BlockBuffer(Box<[u8]>);

impl BlockBuffer {
    fn new() -> Self {
        Self(vec![0; BLOCK_SIZE].into_boxed_slice())
    }
}

// Scramble the buffer on drop to prevent leaving decrypted data in memory past the buffer
// lifetime.
impl Drop for BlockBuffer {
    fn drop(&mut self) {
        self.0.zeroize()
    }
}

impl Deref for BlockBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BlockBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Data for a block that's been loaded into memory and decrypted.
struct OpenBlock {
    parent_name: Option<BlockName>,
    seq: u32,
    kind: BlockKind,
    id: BlockId,
    content: BlockBuffer,
    position: usize,
}

impl OpenBlock {
    // Create trunk (non-head) open block.
    fn trunk(parent_name: BlockName, seq: u32, id: BlockId, content: BlockBuffer) -> Self {
        Self {
            parent_name: Some(parent_name),
            seq,
            kind: BlockKind::Trunk,
            id,
            content,
            position: 0,
        }
    }

    fn nonce_index(&self) -> u32 {
        match self.kind {
            BlockKind::Head => 0,
            BlockKind::Trunk => self.seq,
        }
    }

    fn next_seq(&self) -> u32 {
        match self.kind {
            BlockKind::Head => 1,
            BlockKind::Trunk => {
                // TODO: should we return an error instead?
                self.seq.checked_add(1).expect("too many blocks per blob")
            }
        }
    }

    // Reads data from the current block and advances the internal cursor. Returns the number of
    // bytes actual read.
    fn read(&mut self, buffer: &mut [u8]) -> usize {
        let n = (self.content.len() - self.position).min(buffer.len());
        buffer[..n].copy_from_slice(&self.content[self.position..self.position + n]);
        self.position += n;

        n
    }

    // Writes to the current block and advances the internal cursor. Returns the number of bytes
    // actually written.
    fn write(&mut self, buffer: &[u8]) -> usize {
        let n = (self.content.len() - self.position).min(buffer.len());
        self.content[self.position..self.position + n].copy_from_slice(&buffer[..n]);
        self.position += n;

        if n > 0 {
            self.id.version = BlockVersion::random();
        }

        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{distributions::Standard, Rng};

    #[ignore]
    #[tokio::test]
    async fn root_blob() {
        let pool = init_db().await;
        let secret_key = SecretKey::random();

        let name = BlockName::random();

        // Create a blob spanning 2.5 blocks
        let orig_content: Vec<u8> = rand::thread_rng()
            .sample_iter(Standard)
            .take(5 * BLOCK_SIZE / 2)
            .collect();

        let mut blob = Blob::create(pool.clone(), secret_key.clone(), None, 0, name)
            .await
            .unwrap();
        blob.write(&orig_content[..]).await.unwrap();
        blob.flush().await.unwrap();
        drop(blob);

        // Re-open the blob and read its contents.
        let mut blob = Blob::open(pool.clone(), secret_key.clone(), None, 0, todo!())
            .await
            .unwrap();

        // Read it in chunks of this size.
        let chunk_size = 1024;
        let mut read_contents = vec![0; chunk_size];
        let mut read_len = 0;

        loop {
            let len = blob.read(&mut read_contents[read_len..]).await.unwrap();
            if len == 0 {
                break; // done
            }

            read_len += len;
            read_contents.resize(read_contents.len() + chunk_size, 0);
        }

        assert_eq!(&read_contents[..read_len], &orig_content[..])
    }

    async fn init_db() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        block::init(&pool).await.unwrap();
        pool
    }
}

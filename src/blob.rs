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
use std::convert::TryInto;

pub struct Blob {
    pool: db::Pool,
    secret_key: SecretKey,
    nonce_sequence: NonceSequence,
    current_block: OpenBlock,
}

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

        let mut content = vec![0; BLOCK_SIZE];
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
        let mut content = vec![0; BLOCK_SIZE];
        content[..position].copy_from_slice(nonce_sequence.prefix());

        let current_block = OpenBlock {
            parent_name: directory_name,
            seq: directory_seq,
            kind: BlockKind::Head,
            id: BlockId::random(),
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
            self.fetch_next_block().await?;
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

        // Write the block to the index unless this is the root blob.
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

    async fn fetch_next_block(&mut self) -> Result<(), Error> {
        let parent_name = if let Some(name) = self.current_block.parent_name {
            name
        } else {
            // Only the head block of the root blob can have no parent.
            assert_eq!(self.current_block.seq, 0);
            assert_eq!(self.current_block.kind, BlockKind::Head);

            self.current_block.id.name
        };

        let seq = self.current_block.next_seq();

        let mut content = vec![0; BLOCK_SIZE];

        let child_tag = ChildTag::new(&self.secret_key, &parent_name, seq, BlockKind::Normal);
        let id = index::get(&self.pool, &child_tag).await;
        let id = match id {
            Ok(id) => {
                // existing block
                let nonce = self.nonce_sequence.get(seq);
                let aad = id.to_array(); // "additional associated data"
                let auth_tag = block::read(&self.pool, &id, &mut content).await?;

                let cipher = Cipher::new(self.secret_key.as_array());
                cipher.decrypt_in_place_detached(&nonce, &aad, &mut content, &auth_tag)?;

                id
            }
            Err(Error::BlockIdNotFound) => BlockId::random(), // new block
            Err(error) => return Err(error),
        };

        self.current_block = OpenBlock {
            parent_name: Some(parent_name),
            seq,
            kind: BlockKind::Normal,
            id,
            content,
            position: 0,
        };

        Ok(())
    }
}

// Data for a block that's been loaded into memory and decrypted.
struct OpenBlock {
    parent_name: Option<BlockName>,
    seq: u32,
    kind: BlockKind,
    id: BlockId,
    content: Vec<u8>,
    position: usize,
}

impl OpenBlock {
    fn nonce_index(&self) -> u32 {
        match self.kind {
            BlockKind::Head => 0,
            BlockKind::Normal => self.seq,
        }
    }

    fn next_seq(&self) -> u32 {
        match self.kind {
            BlockKind::Head => 1,
            BlockKind::Normal => {
                // TODO: should we return an error instead?
                self.seq.checked_add(1).expect("too many blocks per blob")
            }
        }
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

use crate::{
    block::{BlockId, BlockName, BlockStore, BlockVersion, BLOCK_SIZE},
    crypto::{
        aead::{AeadInPlace, NewAead},
        Cipher, NonceSequence, SecretKey,
    },
    error::Error,
    index::{BlockKind, ChildTag, Index},
};
use std::sync::Arc;

pub struct Blob {
    block_store: Arc<BlockStore>,
    index: Arc<Index>,
    secret_key: SecretKey,
    nonce_sequence: NonceSequence,
    current_block: BlockCursor,
}

impl Blob {
    /// Opens an existing blob.
    pub async fn open(
        _block_store: Arc<BlockStore>,
        _index: Arc<Index>,
        _secret_key: SecretKey,
        _id: BlockId,
    ) -> Result<Self, Error> {
        todo!()
    }

    /// Creates new blob.
    pub async fn create(
        block_store: Arc<BlockStore>,
        index: Arc<Index>,
        secret_key: SecretKey,
        directory_name: Option<BlockName>,
    ) -> Self {
        Self {
            block_store,
            index,
            secret_key,
            nonce_sequence: NonceSequence::random(),
            current_block: BlockCursor::new(directory_name, 0, BlockKind::Head),
        }
    }

    /// Writes `buffer` into this blob, advancing the blob's internal cursor.
    pub async fn write(&mut self, mut buffer: &[u8]) -> Result<(), Error> {
        loop {
            let len = self.current_block.write(buffer);
            buffer = &buffer[len..];

            if buffer.is_empty() {
                break;
            }

            // TODO: Fetch next block
        }

        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), Error> {
        todo!()
    }

    async fn write_current_block(&self) -> Result<(), Error> {
        let cipher = Cipher::new(self.secret_key.as_array());
        let nonce = self.nonce_sequence.get(self.current_block.seq);
        let aad = self.current_block.id.to_array(); // "additional associated data"

        // Read the plaintext into the buffer.
        let mut buffer = self.current_block.content.clone();

        // Encrypt in place.
        let auth_tag = cipher.encrypt_in_place_detached(&nonce, &aad, &mut buffer)?;

        // Write the block to the block store.
        self.block_store
            .write(&self.current_block.id, &buffer, &auth_tag)
            .await?;

        // Write the block to the index unless this is the root blob.
        if let Some(parent_name) = &self.current_block.parent_name {
            let child_tag = ChildTag::new(
                &self.secret_key,
                parent_name,
                self.current_block.seq,
                self.current_block.kind,
            );
            self.index
                .insert(&self.current_block.id, &child_tag)
                .await?;
        }

        Ok(())
    }
}

struct BlockCursor {
    parent_name: Option<BlockName>,
    seq: u32,
    kind: BlockKind,
    id: BlockId,
    content: Vec<u8>,
    position: usize,
}

impl BlockCursor {
    fn new(parent_name: Option<BlockName>, seq: u32, kind: BlockKind) -> Self {
        let id = BlockId {
            name: BlockName::random(),
            version: BlockVersion::random(),
        };

        Self {
            kind,
            parent_name,
            id,
            seq,
            content: vec![0; BLOCK_SIZE],
            position: 0,
        }
    }

    // Writes to the current block and advances the internal cursor. Returns the number of bytes
    // actually written.
    fn write(&mut self, buffer: &[u8]) -> usize {
        let n = (self.content.len() - self.position).min(buffer.len());
        self.content[self.position..self.position + n].copy_from_slice(&buffer[..n]);
        self.position += n;
        n
    }
}

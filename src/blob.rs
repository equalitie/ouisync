use crate::{
    block::{BlockId, BlockName, BlockStore, BlockVersion, BLOCK_SIZE},
    crypto::{NonceSequence, SecretKey},
    error::Error,
    index::Index,
};
use std::sync::Arc;

pub struct Blob {
    block_store: Arc<BlockStore>,
    index: Arc<Index>,
    secret_key: SecretKey,
    nonce_sequence: NonceSequence,
    cursor: Cursor,
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
    ) -> Self {
        Self {
            block_store,
            index,
            secret_key,
            nonce_sequence: NonceSequence::random(),
            cursor: Cursor::new(),
        }
    }

    pub async fn flush(&mut self) -> Result<(), Error> {}

    async fn write_block(&self) -> Result<(), Error> {}
}

struct Cursor {
    seq: u64,
    id: BlockId,
    content: Vec<u8>,
    position: usize,
}

impl Cursor {
    fn new() -> Self {
        let id = BlockId {
            name: BlockName::random(),
            version: BlockVersion::random(),
        };

        Self {
            id,
            seq: 0,
            content: vec![0; BLOCK_SIZE],
            position: 0,
        }
    }
}

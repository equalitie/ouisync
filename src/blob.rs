use crate::{
    block::{BlockId, BlockStore},
    crypto::SecretKey,
    error::Error,
    index::Index,
};
use std::sync::Arc;

pub struct Blob {
    block_store: Arc<BlockStore>,
    index: Arc<Index>,
    block_seq: usize,
    block_content: Vec<u8>,
    block_offset: usize,
}

impl Blob {
    pub async fn open(
        block_store: Arc<BlockStore>,
        index: Arc<Index>,
        secret_key: &SecretKey,
        id: BlockId,
    ) -> Result<Self, Error> {
        todo!()
    }
}

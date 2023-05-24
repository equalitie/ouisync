use crate::crypto::{Digest, Hash, Hashable};
use serde::{Deserialize, Serialize};
use std::{array::TryFromSliceError, fmt};

mod store;
pub(crate) mod tracker;

pub(crate) use self::{
    store::{count, exists, read, remove, write, BlockNonce, BLOCK_NONCE_SIZE},
    tracker::{BlockTracker, BlockTrackerClient},
};

/// Block size in bytes.
pub const BLOCK_SIZE: usize = 32 * 1024;

/// Size of the block db record in bytes.
pub(crate) const BLOCK_RECORD_SIZE: u64 =
    BLOCK_SIZE as u64 + BlockId::SIZE as u64 + BLOCK_NONCE_SIZE as u64;

/// Unique id of a block.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize, Debug)]
#[repr(transparent)]
pub struct BlockId(Hash);

impl BlockId {
    pub(crate) const SIZE: usize = Hash::SIZE;

    pub(crate) fn from_content(content: &[u8]) -> Self {
        Self(content.hash())
    }
}

impl AsRef<[u8]> for BlockId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl TryFrom<&'_ [u8]> for BlockId {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Hash::try_from(slice).map(Self)
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Hashable for BlockId {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.0.update_hash(state)
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(BlockId);

#[cfg(test)]
derive_rand_for_wrapper!(BlockId);

#[derive(Clone)]
pub(crate) struct BlockData {
    pub content: Box<[u8]>,
    pub id: BlockId,
}

impl From<Box<[u8]>> for BlockData {
    fn from(content: Box<[u8]>) -> Self {
        let id = BlockId::from_content(&content);
        Self { content, id }
    }
}

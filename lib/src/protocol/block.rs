use crate::crypto::{Digest, Hash, Hashable};
use rand::{distributions::Standard, prelude::Distribution, Rng};
use serde::{Deserialize, Serialize};
use std::{
    array::TryFromSliceError,
    fmt,
    ops::{Deref, DerefMut},
};
use zeroize::Zeroize;

/// Block size in bytes.
pub const BLOCK_SIZE: usize = 32 * 1024;

/// Size of the block db record in bytes.
pub(crate) const BLOCK_RECORD_SIZE: u64 =
    BLOCK_SIZE as u64 + BlockId::SIZE as u64 + BLOCK_NONCE_SIZE as u64;

pub(crate) const BLOCK_NONCE_SIZE: usize = 32;
pub(crate) type BlockNonce = [u8; BLOCK_NONCE_SIZE];

/// Unique id of a block.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
pub struct BlockId(Hash);

impl BlockId {
    pub(crate) const SIZE: usize = Hash::SIZE;

    pub(crate) fn from_content(content: &BlockContent) -> Self {
        Self(content.0[..].hash())
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

impl fmt::Debug for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
pub(crate) struct Block {
    pub id: BlockId,
    pub content: BlockContent,
    pub nonce: BlockNonce,
}

impl Block {
    pub fn new(content: BlockContent, nonce: BlockNonce) -> Self {
        let id = BlockId::from_content(&content);
        Self { id, content, nonce }
    }
}

impl Distribution<Block> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Block {
        Block::new(rng.gen(), rng.gen())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct BlockContent(Box<[u8]>);

impl BlockContent {
    pub fn new() -> Self {
        Self::default()
    }

    // Read data from `offset` of the buffer into a fixed-length array.
    //
    // # Panics
    //
    // Panics if the remaining length after `offset` is less than `N`.
    pub fn read_array<const N: usize>(&self, offset: usize) -> [u8; N] {
        self[offset..offset + N].try_into().unwrap()
    }

    // Read data from `offset` of the buffer into a `u64`.
    //
    // # Panics
    //
    // Panics if the remaining length is less than `size_of::<u64>()`
    pub fn read_u64(&self, offset: usize) -> u64 {
        u64::from_le_bytes(self.read_array(offset))
    }

    // Read data from offset into `dst`.
    pub fn read(&self, offset: usize, dst: &mut [u8]) {
        dst.copy_from_slice(&self.0[offset..offset + dst.len()]);
    }

    // Write a `u64` at `offset` into the buffer.
    pub fn write_u64(&mut self, offset: usize, value: u64) {
        let bytes = value.to_le_bytes();
        self.write(offset, &bytes[..]);
    }

    // Writes data from `dst` into the buffer.
    pub fn write(&mut self, offset: usize, src: &[u8]) {
        self.0[offset..offset + src.len()].copy_from_slice(src);
    }
}

impl Default for BlockContent {
    fn default() -> Self {
        Self(vec![0; BLOCK_SIZE].into_boxed_slice())
    }
}

// Scramble the buffer on drop to prevent leaving decrypted data in memory past the buffer
// lifetime.
impl Drop for BlockContent {
    fn drop(&mut self) {
        self.0.zeroize()
    }
}

impl Deref for BlockContent {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for BlockContent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Distribution<BlockContent> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlockContent {
        let mut content = vec![0; BLOCK_SIZE].into_boxed_slice();
        rng.fill(&mut content[..]);

        BlockContent(content)
    }
}

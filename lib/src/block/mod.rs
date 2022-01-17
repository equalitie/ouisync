use crate::crypto::{Digest, Hash, Hashable};
use serde::{Deserialize, Serialize};
use std::{array::TryFromSliceError, fmt};

#[cfg(test)]
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};

mod store;

#[cfg(test)]
pub(crate) use self::store::exists;
pub(crate) use self::store::{init, read, write, BlockNonce};

/// Block size in bytes.
pub const BLOCK_SIZE: usize = 32 * 1024;

/// Unique id of a block.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize, Debug)]
#[repr(transparent)]
pub struct BlockId(Hash);

impl BlockId {
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

#[cfg(test)]
impl Distribution<BlockId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlockId {
        use generic_array::GenericArray;

        let inner: [u8; Hash::SIZE] = rng.gen();
        let inner: GenericArray<_, _> = inner.into();
        let hash = Hash::from(inner);

        BlockId(hash)
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(BlockId);

use crate::format;
use generic_array::{sequence::GenericSequence, typenum::Unsigned, GenericArray};
use serde::{Deserialize, Serialize};
use sha3::{digest::Digest, Sha3_256};
use std::{array::TryFromSliceError, fmt};
use zeroize::Zeroize;

/// Wrapper for a 256-bit hash digest, for convenience. Also implements friendly formatting.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Hash(Inner);

impl Hash {
    pub const SIZE: usize = <Inner as GenericSequence<_>>::Length::USIZE;

    pub fn as_array(&self) -> &Inner {
        &self.0
    }
}

impl From<Inner> for Hash {
    fn from(inner: Inner) -> Self {
        Hash(inner)
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

impl TryFrom<&'_ [u8]> for Hash {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let slice: [u8; Self::SIZE] = slice.try_into()?;
        Ok(Self(slice.into()))
    }
}

impl From<Hash> for [u8; Hash::SIZE] {
    fn from(hash: Hash) -> [u8; Hash::SIZE] {
        hash.0.into()
    }
}

impl Zeroize for Hash {
    fn zeroize(&mut self) {
        self.0.zeroize()
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(Hash);

type Inner = GenericArray<u8, <Sha3_256 as Digest>::OutputSize>;

/// Trait for types that can be cryptographically hashed.
pub trait Hashable {
    fn hash(&self) -> Hash;
}

impl Hashable for &'_ [u8] {
    fn hash(&self) -> Hash {
        Sha3_256::digest(self).into()
    }
}

impl Hashable for u64 {
    fn hash(&self) -> Hash {
        (&self.to_le_bytes()[..]).hash()
    }
}

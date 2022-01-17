use crate::format;
use generic_array::{
    sequence::GenericSequence,
    typenum::{Unsigned, U32},
    GenericArray,
};
use serde::{Deserialize, Serialize};
use sha3::Sha3_256;
use std::{array::TryFromSliceError, fmt, slice};
use zeroize::Zeroize;

pub use sha3::digest::Digest;

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

impl Hashable for Hash {
    fn update_hash<H: Digest>(&self, h: &mut H) {
        self.as_ref().update_hash(h)
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(Hash);

type Inner = GenericArray<u8, <Sha3_256 as Digest>::OutputSize>;

/// Similar to std::hash::Hash, but for cryptographic hashes.
pub trait Hashable {
    // Update the hash state.
    fn update_hash<H: Digest>(&self, h: &mut H);

    // Hash self using the given hashing algorithm.
    fn hash_with<H>(&self) -> Hash
    where
        H: Digest<OutputSize = U32>,
    {
        let mut h = H::new();
        self.update_hash(&mut h);
        h.finalize().into()
    }

    // Hash self using the default hashing algorithm (SHA3-256).
    fn hash(&self) -> Hash {
        self.hash_with::<Sha3_256>()
    }
}

impl Hashable for [u8] {
    fn update_hash<H: Digest>(&self, h: &mut H) {
        h.update(self)
    }
}

impl Hashable for u8 {
    fn update_hash<H: Digest>(&self, h: &mut H) {
        slice::from_ref(self).update_hash(h)
    }
}

impl Hashable for u32 {
    fn update_hash<H: Digest>(&self, h: &mut H) {
        self.to_le_bytes().update_hash(h)
    }
}

impl Hashable for u64 {
    fn update_hash<H: Digest>(&self, h: &mut H) {
        self.to_le_bytes().update_hash(h)
    }
}

impl<T0, T1> Hashable for (T0, T1)
where
    T0: Hashable,
    T1: Hashable,
{
    fn update_hash<H: Digest>(&self, h: &mut H) {
        self.0.update_hash(h);
        self.1.update_hash(h);
    }
}

impl<'a, T> Hashable for &'a T
where
    T: Hashable + ?Sized,
{
    fn update_hash<H: Digest>(&self, h: &mut H) {
        (*self).update_hash(h)
    }
}

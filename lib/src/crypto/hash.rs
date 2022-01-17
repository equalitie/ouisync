pub use sha3::digest::Digest;

use crate::format;
use generic_array::{typenum::U32, GenericArray};
use serde::{Deserialize, Serialize};
use sha3::Sha3_256;
use std::{
    array::TryFromSliceError,
    collections::{BTreeMap, BTreeSet},
    fmt, slice,
};
use zeroize::Zeroize;

/// Wrapper for a 256-bit hash digest, for convenience. Also implements friendly formatting.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Hash([u8; Self::SIZE]);

impl Hash {
    pub const SIZE: usize = 32;
}

impl From<[u8; Self::SIZE]> for Hash {
    fn from(array: [u8; Self::SIZE]) -> Self {
        Hash(array)
    }
}

impl From<GenericArray<u8, U32>> for Hash {
    fn from(array: GenericArray<u8, U32>) -> Self {
        Hash(array.into())
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
        Ok(Self(slice.try_into()?))
    }
}

impl From<Hash> for [u8; Hash::SIZE] {
    fn from(hash: Hash) -> [u8; Hash::SIZE] {
        hash.0
    }
}

impl Zeroize for Hash {
    fn zeroize(&mut self) {
        self.0.zeroize()
    }
}

impl Hashable for Hash {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.as_ref().update_hash(state)
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(Hash);

#[cfg(test)]
derive_rand_for_wrapper!(Hash);

/// Similar to std::hash::Hash, but for cryptographic hashes.
pub trait Hashable {
    // Update the hash state.
    fn update_hash<S: Digest>(&self, state: &mut S);

    // This is needed due to lack of specialization in stable rust.
    fn update_hash_slice<S>(slice: &[Self], state: &mut S)
    where
        S: Digest,
        Self: Sized,
    {
        for item in slice {
            item.update_hash(state)
        }
    }

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

impl Hashable for u8 {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        state.update(slice::from_ref(self))
    }

    fn update_hash_slice<S: Digest>(slice: &[Self], state: &mut S) {
        state.update(slice)
    }
}

impl Hashable for u32 {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        state.update(&self.to_le_bytes())
    }
}

impl Hashable for u64 {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        state.update(&self.to_le_bytes())
    }
}

impl<T> Hashable for [T]
where
    T: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        (self.len() as u64).update_hash(state);
        Hashable::update_hash_slice(self, state);
    }
}

impl<T> Hashable for Vec<T>
where
    T: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.as_slice().update_hash(state);
    }
}

impl<K, V> Hashable for BTreeMap<K, V>
where
    K: Hashable,
    V: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        (self.len() as u64).update_hash(state);
        for (key, value) in self {
            key.update_hash(state);
            value.update_hash(state);
        }
    }
}

impl<T> Hashable for BTreeSet<T>
where
    T: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        (self.len() as u64).update_hash(state);
        for item in self {
            item.update_hash(state);
        }
    }
}

// NOTE: `Hashable` is purposefully not implemented for `HashMap` / `HashSet` because the resulting
// hash would be dependent on the iteration order which in case of `HashMap` / `HashSet` is often
// random. Thus two maps/set that compare as equal would produce different hashes.

impl<T0, T1> Hashable for (T0, T1)
where
    T0: Hashable,
    T1: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.0.update_hash(state);
        self.1.update_hash(state);
    }
}

impl<T0, T1, T2> Hashable for (T0, T1, T2)
where
    T0: Hashable,
    T1: Hashable,
    T2: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.0.update_hash(state);
        self.1.update_hash(state);
        self.2.update_hash(state);
    }
}

impl<'a, T> Hashable for &'a T
where
    T: Hashable + ?Sized,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        (**self).update_hash(state);
    }
}

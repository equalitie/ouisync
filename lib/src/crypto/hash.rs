pub use blake3::traits::digest::Digest;

use crate::format;
use generic_array::{typenum::U32, GenericArray};
use serde::{Deserialize, Serialize};
use std::{
    array::TryFromSliceError,
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::Deref,
    slice,
};

#[cfg(test)]
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};

/// Wrapper for a 256-bit hash digest. Also implements friendly formatting.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(from = "[u8; Self::SIZE]", into = "[u8; Self::SIZE]")]
pub struct Hash(blake3::Hash);

impl Hash {
    pub const SIZE: usize = blake3::OUT_LEN;
}

impl From<[u8; Self::SIZE]> for Hash {
    fn from(array: [u8; Self::SIZE]) -> Self {
        Hash(array.into())
    }
}

impl From<GenericArray<u8, U32>> for Hash {
    fn from(array: GenericArray<u8, U32>) -> Self {
        let array: [u8; Self::SIZE] = array.into();
        Hash(array.into())
    }
}

impl TryFrom<&'_ [u8]> for Hash {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let array: [u8; Self::SIZE] = slice.try_into()?;
        Ok(Self(array.into()))
    }
}

impl From<Hash> for [u8; Hash::SIZE] {
    fn from(hash: Hash) -> [u8; Hash::SIZE] {
        hash.0.into()
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes().as_slice()
    }
}

impl PartialOrd for Hash {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Hash {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:x}")
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:8x}")
    }
}

impl fmt::LowerHex for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, self.as_ref())
    }
}

impl Hashable for Hash {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.as_ref().update_hash(state)
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(Hash);

#[cfg(test)]
impl Distribution<Hash> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Hash {
        let array: [u8; Hash::SIZE] = rng.gen();
        Hash(array.into())
    }
}

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

    // Hash self using the default hashing algorithm (BLAKE3).
    fn hash(&self) -> Hash {
        self.hash_with::<blake3::Hasher>()
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
        state.update(self.to_le_bytes())
    }
}

impl Hashable for u64 {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        state.update(self.to_le_bytes())
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

impl<T, const N: usize> Hashable for [T; N]
where
    T: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.as_slice().update_hash(state)
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

macro_rules! impl_hashable_for_tuple {
    ($($name:ident)+) => {
        impl<$($name: Hashable),+> Hashable for ($($name,)+) {
            #[allow(non_snake_case)]
            fn update_hash<S: Digest>(&self, state: &mut S) {
                let ($($name,)+) = self;
                $($name.update_hash(state);)+
            }
        }
    }
}

impl_hashable_for_tuple!(T0 T1);
impl_hashable_for_tuple!(T0 T1 T2);
impl_hashable_for_tuple!(T0 T1 T2 T3);

/// Wrapper that caches the hash of the inner value.
pub(crate) struct CacheHash<T> {
    owner: T,
    hash: Hash,
}

impl<T> CacheHash<T> {
    pub fn into_inner(self) -> T {
        self.owner
    }
}

impl<T> Deref for CacheHash<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

impl<T> From<T> for CacheHash<T>
where
    T: Hashable,
{
    fn from(owner: T) -> Self {
        let hash = owner.hash();
        Self { owner, hash }
    }
}

impl<T> Hashable for CacheHash<T>
where
    T: Hashable,
{
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.owner.update_hash(state)
    }

    fn hash(&self) -> Hash {
        self.hash
    }
}

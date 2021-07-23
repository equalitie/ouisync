use crate::format;
/// BlobId is used to identify a blob in a directory
use serde::{Deserialize, Serialize};

use rand::{
    distributions::{Distribution, Standard},
    Rng,
};

use std::{
    array::TryFromSliceError,
    convert::{TryFrom, TryInto},
    fmt,
};

// TODO: At some point we should probably have a dedicated byte array class to avoid duplicating
// this code (here, then in Hash, ReplicaId, RuntimeId,..?).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct BlobId([u8; 32]);

impl BlobId {
    /// Generate a random id using the default RNG ([`rand::thread_rng`]).
    pub fn random() -> Self {
        rand::thread_rng().gen()
    }

    pub fn random_with_rng<R: Rng>(rng: &mut R) -> Self {
        rng.gen()
    }

    pub fn zero() -> Self {
        BlobId([0; 32])
    }
}

impl AsRef<[u8]> for BlobId {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl Distribution<BlobId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> BlobId {
        BlobId(self.sample(rng))
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

impl TryFrom<&'_ [u8]> for BlobId {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(slice.try_into()?))
    }
}

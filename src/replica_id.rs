use crate::format;
use serde::{Serialize, Deserialize};
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use std::fmt;

/// Size of replica ID in bytes.
pub const REPLICA_ID_SIZE: usize = 16;

/// Unique name of a block which doesn't change when the block is modified.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ReplicaId([u8; REPLICA_ID_SIZE]);

impl ReplicaId {
    /// Generate a random name using the default RNG ([`rand::thread_rng`]).
    pub fn random() -> Self {
        rand::thread_rng().gen()
    }
}

impl AsRef<[u8]> for ReplicaId {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl Distribution<ReplicaId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> ReplicaId {
        ReplicaId(self.sample(rng))
    }
}

impl fmt::Display for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

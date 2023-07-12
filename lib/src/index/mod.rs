mod proof;
mod summary;

#[cfg(test)]
pub(crate) mod test_utils;

pub(crate) use self::{
    proof::{Proof, ProofError, UntrustedProof},
    summary::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary},
};

use crate::{crypto::sign::PublicKey, version_vector::VersionVector};

pub(crate) type SnapshotId = u32;

/// Operation on version vector
#[derive(Clone, Copy, Debug)]
pub(crate) enum VersionVectorOp<'a> {
    IncrementLocal,
    Merge(&'a VersionVector),
}

impl VersionVectorOp<'_> {
    pub fn apply(self, local_id: &PublicKey, target: &mut VersionVector) {
        match self {
            Self::IncrementLocal => {
                target.increment(*local_id);
            }
            Self::Merge(other) => {
                target.merge(other);
            }
        }
    }
}

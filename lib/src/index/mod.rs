mod node;
mod proof;

#[cfg(test)]
pub(crate) use self::node::test_utils as node_test_utils;
pub(crate) use self::{
    node::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary},
    proof::{Proof, ProofError, UntrustedProof},
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

use super::{MultiBlockPresence, NodeState, Proof, Summary, EMPTY_INNER_HASH};
use crate::{
    crypto::sign::{Keypair, PublicKey},
    version_vector::VersionVector,
    versioned::{BranchItem, Versioned},
};

pub(crate) type SnapshotId = u32;

const EMPTY_SNAPSHOT_ID: SnapshotId = 0;

#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub summary: Summary,
}

impl RootNode {
    /// Creates a root node with no children without storing it in the database.
    pub fn empty(writer_id: PublicKey, write_keys: &Keypair) -> Self {
        let proof = Proof::new(
            writer_id,
            VersionVector::new(),
            *EMPTY_INNER_HASH,
            write_keys,
        );

        Self {
            snapshot_id: EMPTY_SNAPSHOT_ID,
            proof,
            summary: Summary {
                state: NodeState::Approved,
                block_presence: MultiBlockPresence::Full,
            },
        }
    }
}

impl Versioned for RootNode {
    fn version_vector(&self) -> &VersionVector {
        &self.proof.version_vector
    }
}

impl BranchItem for RootNode {
    fn branch_id(&self) -> &PublicKey {
        &self.proof.writer_id
    }
}

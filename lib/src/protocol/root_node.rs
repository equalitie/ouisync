use super::{Proof, Summary};
use crate::{
    crypto::sign::PublicKey,
    version_vector::VersionVector,
    versioned::{BranchItem, Versioned},
};

pub(crate) type SnapshotId = u32;

#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub summary: Summary,
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

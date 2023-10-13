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

/// Kind of root node.
#[derive(Eq, PartialEq, Debug)]
pub(crate) enum RootNodeKind {
    /// Published nodes have their version vector strictly greater than any previous node
    /// in the same branch. Only published nodes are announced to other replicas.
    Published,
    /// Draft nodes have their version vector equal to that of the previous node in the same
    /// branch. Draft nodes are neither announced to nor accepted from other replicas.
    Draft,
}

/// What kind of nodes to load and/or create.
pub(crate) enum RootNodeFilter {
    /// Only published nodes.
    Published,
    /// Published and draft nodes.
    Any,
}

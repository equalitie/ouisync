use super::{Proof, Summary};

pub(crate) type SnapshotId = u32;

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub proof: Proof,
    pub summary: Summary,
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

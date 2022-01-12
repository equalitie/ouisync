use super::node::InnerNodeMap;
use crate::crypto::{sign::PublicKey, Hash, Hashable};
use serde::{Deserialize, Serialize};

/// Information that prove that a snapshot was created by a replica that has write access to the
/// repository.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub writer_id: PublicKey,
    pub hash: Hash,
    // TODO: signature
}

impl Proof {
    pub fn new(writer_id: PublicKey, hash: Hash) -> Self {
        Self { writer_id, hash }
    }

    /// Proof attached to the first root node of a branch.
    pub fn first(writer_id: PublicKey) -> Self {
        Self::new(writer_id, InnerNodeMap::default().hash())
    }
}

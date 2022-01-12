use crate::crypto::{sign::PublicKey, Hash};

/// Information that prove that a snapshot was created by a replica that has write access to the
/// repository.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) struct Proof {
    writer_id: PublicKey,
    hash: Hash,
    // TODO: signature
}

impl Proof {
    pub fn new(writer_id: PublicKey, hash: Hash) -> Self {
        Self { writer_id, hash }
    }

    // /// Proof attached to the first root node of a branch.
    // pub fn first(writer_id: PublicKey) -> Self {

    // }

    pub fn writer_id(&self) -> &PublicKey {
        &self.writer_id
    }

    pub fn hash(&self) -> &Hash {
        &self.hash
    }
}

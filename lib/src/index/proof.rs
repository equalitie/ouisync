use super::node::InnerNodeMap;
use crate::{
    crypto::{
        sign::{Keypair, PublicKey, Signature},
        Hash, Hashable,
    },
    repository::RepositoryId,
};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use thiserror::Error;

/// Information that prove that a snapshot was created by a replica that has write access to the
/// repository.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub writer_id: PublicKey,
    pub hash: Hash,
    pub signature: Signature,
}

impl Proof {
    pub fn verify(self, repository_id: &RepositoryId) -> Result<Verified, ProofError> {
        let signature_material = signature_material(&self.writer_id, &self.hash);
        if repository_id
            .write_public_key()
            .verify(&signature_material, &self.signature)
        {
            Ok(Verified(self))
        } else {
            Err(ProofError)
        }
    }

    /// Assume the proof is verified without actually verifying it and convert it into `Verified`.
    ///
    /// Use only for proofs loaded from the local database, never for ones received from remote
    /// replicas.
    pub fn assume_verified(self) -> Verified {
        Verified(self)
    }
}

impl From<Verified> for Proof {
    fn from(verified: Verified) -> Self {
        verified.0
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) struct Verified(Proof);

impl Verified {
    pub fn new(writer_id: PublicKey, hash: Hash, write_keys: &Keypair) -> Self {
        let signature_material = signature_material(&writer_id, &hash);
        let signature = write_keys.sign(&signature_material);

        Self(Proof {
            writer_id,
            hash,
            signature,
        })
    }

    /// Proof for the first snapshot of a newly created branch.
    pub fn first(writer_id: PublicKey, write_keys: &Keypair) -> Self {
        Self::new(writer_id, InnerNodeMap::default().hash(), write_keys)
    }

    /// Create proof for the next snapshot version.
    pub fn next(self, hash: Hash, write_keys: &Keypair) -> Self {
        Self::new(self.0.writer_id, hash, write_keys)
    }
}

impl Deref for Verified {
    type Target = Proof;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn signature_material(writer_id: &PublicKey, hash: &Hash) -> [u8; PublicKey::SIZE + Hash::SIZE] {
    let mut array = [0; PublicKey::SIZE + Hash::SIZE];
    array[..PublicKey::SIZE].copy_from_slice(writer_id.as_ref());
    array[PublicKey::SIZE..].copy_from_slice(hash.as_ref());
    array
}

#[derive(Debug, Error)]
#[error("proof is invalid")]
pub(crate) struct ProofError;

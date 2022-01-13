use super::node::EMPTY_INNER_HASH;
use crate::{
    crypto::{
        sign::{Keypair, PublicKey, Signature},
        Hash,
    },
    repository::RepositoryId,
    version_vector::VersionVector,
};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use thiserror::Error;

/// Information that prove that a snapshot was created by a replica that has write access to the
/// repository.
#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) struct Proof(UntrustedProof);

impl Proof {
    /// Create new proof signed with the given write keys.
    pub fn new(
        writer_id: PublicKey,
        version_vector: VersionVector,
        hash: Hash,
        write_keys: &Keypair,
    ) -> Self {
        let signature_material = signature_material(&writer_id, &version_vector, &hash);
        let signature = write_keys.sign(&signature_material);

        Self::new_unchecked(writer_id, version_vector, hash, signature)
    }

    /// Create new proof form a pre-existing signature without checking whether the signature
    /// is valid. Use only when loading proofs from the local db, never when receiving them from
    /// remote replicas.
    pub fn new_unchecked(
        writer_id: PublicKey,
        version_vector: VersionVector,
        hash: Hash,
        signature: Signature,
    ) -> Self {
        Self(UntrustedProof {
            writer_id,
            version_vector,
            hash,
            signature,
        })
    }

    /// Proof for the first snapshot of a newly created branch.
    pub fn first(writer_id: PublicKey, write_keys: &Keypair) -> Self {
        Self::new(
            writer_id,
            VersionVector::new(),
            *EMPTY_INNER_HASH,
            write_keys,
        )
    }
}

impl Deref for Proof {
    type Target = UntrustedProof;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct UntrustedProof {
    pub writer_id: PublicKey,
    pub version_vector: VersionVector,
    pub hash: Hash,
    pub signature: Signature,
}

impl UntrustedProof {
    pub fn verify(self, repository_id: &RepositoryId) -> Result<Proof, ProofError> {
        let signature_material =
            signature_material(&self.writer_id, &self.version_vector, &self.hash);
        if repository_id
            .write_public_key()
            .verify(&signature_material, &self.signature)
        {
            Ok(Proof(self))
        } else {
            Err(ProofError)
        }
    }
}

impl From<Proof> for UntrustedProof {
    fn from(proof: Proof) -> Self {
        proof.0
    }
}

// TODO: consider an alternative way to do this: iteratively build a SHA2-512 hash of all the
// fields and then use that hash with `sign_prehashed` / `verify_prehashed`. This would avoid the
// potentially expensive serialization and allocation.
fn signature_material(
    writer_id: &PublicKey,
    version_vector: &VersionVector,
    hash: &Hash,
) -> Vec<u8> {
    // unwrap should be OK because serialization into a vector has no reason to ever fail (unless
    // there is a bug).
    bincode::serialize(&(writer_id, version_vector, hash)).unwrap()
}

#[derive(Debug, Error)]
#[error("proof is invalid")]
pub(crate) struct ProofError;

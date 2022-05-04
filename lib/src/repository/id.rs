use crate::crypto::{
    sign::{self, PublicKey, SecretKey},
    Digest, Hash, Hashable,
};
use rand::{rngs::OsRng, CryptoRng, Rng};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(PartialEq, Eq, Clone, Debug, Copy, Serialize, Deserialize)]
#[repr(transparent)]
pub struct RepositoryId(PublicKey);

derive_sqlx_traits_for_byte_array_wrapper!(RepositoryId);

impl RepositoryId {
    pub const SIZE: usize = PublicKey::SIZE;

    // // TODO: Temporarily enabling for non tests as well.
    // //#[cfg(test)]
    pub fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        let sk = SecretKey::generate(rng);
        RepositoryId(PublicKey::from(&sk))
    }

    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }

    /// Hash of this id using the given salt.
    pub fn salted_hash(&self, salt: &[u8]) -> Hash {
        (self, salt).hash()
    }

    pub fn write_public_key(&self) -> &PublicKey {
        &self.0
    }
}

impl FromStr for RepositoryId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(PublicKey::from_str(s)?))
    }
}

impl AsRef<[u8]> for RepositoryId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl TryFrom<&'_ [u8]> for RepositoryId {
    type Error = sign::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(PublicKey::try_from(bytes)?))
    }
}

impl From<PublicKey> for RepositoryId {
    fn from(pk: PublicKey) -> Self {
        Self(pk)
    }
}

impl Hashable for RepositoryId {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.0.update_hash(state)
    }
}

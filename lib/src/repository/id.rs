use crate::crypto::{
    sign::{self, PublicKey, SecretKey},
    Hash,
};
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use serde::{Deserialize, Serialize};
use sha3::{digest::Digest, Sha3_256};
use std::str::FromStr;

#[derive(PartialEq, Eq, Clone, Debug, Copy, Serialize, Deserialize)]
#[repr(transparent)]
pub struct RepositoryId(PublicKey);

derive_sqlx_traits_for_byte_array_wrapper!(RepositoryId);

impl RepositoryId {
    pub const SIZE: usize = PublicKey::SIZE;

    /// Hash of this id using the given salt.
    pub fn salted_hash(&self, salt: &[u8]) -> Hash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.0.as_ref());
        hasher.update(salt);
        hasher.finalize().into()
    }
}

// // TODO: Temporarily enabling for non tests as well.
// //#[cfg(test)]
impl Distribution<RepositoryId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> RepositoryId {
        let sk: SecretKey = rng.gen();
        RepositoryId(PublicKey::from(&sk))
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

impl From<[u8; PublicKey::SIZE]> for RepositoryId {
    fn from(bytes: [u8; PublicKey::SIZE]) -> Self {
        Self(PublicKey::from(bytes))
    }
}

impl From<PublicKey> for RepositoryId {
    fn from(pk: PublicKey) -> Self {
        Self(pk)
    }
}

use crate::{
    crypto::{
        sign::{self, PublicKey, SecretKey},
        Hash,
    },
    format,
};
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use sha3::{digest::Digest, Sha3_256};
use std::{fmt, str::FromStr};

#[derive(PartialEq, Eq, Clone, Debug, Copy)]
pub struct SecretRepositoryId(PublicKey);

derive_sqlx_traits_for_byte_array_wrapper!(SecretRepositoryId);

impl SecretRepositoryId {
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
impl Distribution<SecretRepositoryId> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> SecretRepositoryId {
        let sk: SecretKey = rng.gen();
        SecretRepositoryId(PublicKey::from(&sk))
    }
}

impl FromStr for SecretRepositoryId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(PublicKey::from_str(s)?))
    }
}

impl fmt::LowerHex for SecretRepositoryId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, self.0.as_ref())
    }
}

impl AsRef<[u8]> for SecretRepositoryId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl TryFrom<&'_ [u8]> for SecretRepositoryId {
    type Error = sign::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(PublicKey::try_from(bytes)?))
    }
}

impl From<[u8; PublicKey::SIZE]> for SecretRepositoryId {
    fn from(bytes: [u8; PublicKey::SIZE]) -> Self {
        Self(PublicKey::from(bytes))
    }
}

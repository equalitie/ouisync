use crate::{
    crypto::{
        sign::{self, PublicKey},
        Hash,
    },
    format,
};
use btdht::{InfoHash, INFO_HASH_LEN};
use serde::{Deserialize, Serialize};
use sha3::{digest::Digest, Sha3_256};
use std::{fmt, str::FromStr};

#[derive(PartialEq, Eq, Clone, Debug, Copy)]
pub struct SecretRepositoryId(PublicKey);

derive_sqlx_traits_for_byte_array_wrapper!(SecretRepositoryId);

impl SecretRepositoryId {
    pub const SIZE: usize = PublicKey::SIZE;

    /// Obtain the public id corresponding to this secret id.
    pub fn public(&self) -> PublicRepositoryId {
        // Calculate the info hash by hashing the id with SHA3-256 and taking the first 20 bytes.
        // (bittorrent uses SHA-1 but that is less secure).
        // `unwrap` is OK because the byte slice has the correct length.
        PublicRepositoryId(
            InfoHash::try_from(&self.salted_hash(b"public-id").as_ref()[..INFO_HASH_LEN]).unwrap(),
        )
    }

    /// Hash of this id using the given salt.
    pub fn salted_hash(&self, salt: &[u8]) -> Hash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.0.as_ref());
        hasher.update(salt);
        hasher.finalize().into()
    }
}

// TODO: Temporarily enabling for non tests as well.
//#[cfg(test)]
impl rand::distributions::Distribution<SecretRepositoryId> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, _rng: &mut R) -> SecretRepositoryId {
        let keypair = crate::crypto::sign::Keypair::generate();
        SecretRepositoryId(keypair.public)
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

/// Public repository id can be freely shared with anyone because it's impossible to extract the
/// secret id out of it. It can also be used as the info hash for DHT lookups.
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
pub struct PublicRepositoryId(InfoHash);

impl PublicRepositoryId {
    pub(crate) const SIZE: usize = INFO_HASH_LEN;

    pub(crate) fn to_info_hash(self) -> InfoHash {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn random() -> Self {
        Self(rand::random())
    }
}

impl From<[u8; Self::SIZE]> for PublicRepositoryId {
    fn from(array: [u8; Self::SIZE]) -> Self {
        Self(array.into())
    }
}

impl AsRef<[u8]> for PublicRepositoryId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl fmt::Display for PublicRepositoryId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::LowerHex for PublicRepositoryId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, self.0.as_ref())
    }
}

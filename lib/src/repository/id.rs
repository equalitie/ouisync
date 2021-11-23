use crate::{crypto::Hashable, format};
use btdht::{InfoHash, INFO_HASH_LEN};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Size of SecretRepositoryId in bytes.
pub const SECRET_REPOSITORY_ID_SIZE: usize = 16;

define_byte_array_wrapper! {
    /// Unique secret id of a repository. Only known to replicas sharing the repository.
    pub struct SecretRepositoryId([u8; SECRET_REPOSITORY_ID_SIZE]);
}

derive_rand_for_wrapper!(SecretRepositoryId);
derive_sqlx_traits_for_byte_array_wrapper!(SecretRepositoryId);

impl SecretRepositoryId {
    /// Obtain the public id corresponding to this secret id.
    pub fn public(&self) -> PublicRepositoryId {
        // Calculate the info hash by hashing the id with SHA3-256 and taking the first 20 bytes.
        // (bittorrent uses SHA-1 but that is less secure).
        // `unwrap` is OK because the byte slice has the correct length.
        PublicRepositoryId(
            InfoHash::try_from(&self.as_ref().hash().as_ref()[..INFO_HASH_LEN]).unwrap(),
        )
    }
}

impl FromStr for SecretRepositoryId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; SECRET_REPOSITORY_ID_SIZE];
        hex::decode_to_slice(s, &mut bytes)?;

        Ok(Self(bytes))
    }
}

/// Public repository id can be freely shared with anyone because it's impossible to extract the
/// secret id out of it. It can also be used as the info hash for DHT lookups.
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
pub struct PublicRepositoryId(InfoHash);

impl PublicRepositoryId {
    pub(crate) fn to_info_hash(self) -> InfoHash {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn zero() -> Self {
        Self(InfoHash::from([0; INFO_HASH_LEN]))
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

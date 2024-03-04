use crate::crypto::{
    sign::{self, PublicKey},
    Digest, Hash, Hashable,
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    str::FromStr,
    sync::atomic::{AtomicU32, Ordering},
};

#[derive(PartialEq, Eq, Clone, Debug, Copy, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct RepositoryId(PublicKey);

derive_sqlx_traits_for_byte_array_wrapper!(RepositoryId);

impl RepositoryId {
    pub const SIZE: usize = PublicKey::SIZE;

    #[cfg(test)]
    pub fn generate<R: rand::Rng + rand::CryptoRng>(rng: &mut R) -> Self {
        crate::crypto::sign::Keypair::generate(rng)
            .public_key()
            .into()
    }

    #[cfg(test)]
    pub fn random() -> Self {
        Self::generate(&mut rand::rngs::OsRng)
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
    type Err = sign::ParseError;

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

/// Simple numeric id that is unique only locally. Useful mostly for debugging.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct LocalId(u32);

impl LocalId {
    // `Default` would be misleading here, because each invocation of `default` would create
    // a different value.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for LocalId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::sign::Keypair;

    #[test]
    fn serialize_deserialize() {
        let public_key = Keypair::random().public_key();
        let id = RepositoryId::from(public_key);

        let bytes = public_key.as_ref();

        let serialized_expected = serde_json::to_string(bytes).unwrap();
        let serialized_actual = serde_json::to_string(&id).unwrap();

        assert_eq!(serialized_actual, serialized_expected);

        let deserialized_actual: RepositoryId = serde_json::from_str(&serialized_actual).unwrap();
        assert_eq!(deserialized_actual, id);
    }
}

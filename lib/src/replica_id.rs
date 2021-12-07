use crate::crypto::sign;
use serde;
use std::fmt;

#[derive(
    PartialEq, Eq, Clone, Copy, Hash, Debug, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ReplicaId(sign::PublicKey);

derive_sqlx_traits_for_byte_array_wrapper!(ReplicaId);

impl ReplicaId {
    pub fn starts_with(&self, needle: &[u8]) -> bool {
        self.0.as_ref().starts_with(needle)
    }
}

impl fmt::LowerHex for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

// TODO: Temporarily enabling for non tests as well.
//#[cfg(test)]
impl rand::distributions::Distribution<ReplicaId> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> ReplicaId {
        ReplicaId(self.sample(rng))
    }
}

impl std::fmt::Display for ReplicaId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<[u8]> for ReplicaId {
    fn as_ref(&self) -> &[u8] {
        &self.0.as_ref()
    }
}

impl TryFrom<&'_ [u8]> for ReplicaId {
    type Error = sign::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let pk = sign::PublicKey::try_from(bytes)?;
        Ok(ReplicaId(pk))
    }
}

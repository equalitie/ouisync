use crate::format;
use core::hash::{Hash, Hasher};
use ed25519_dalek as ext;
use ed25519_dalek::Verifier;
use rand::{rngs::OsRng, Rng};
use serde;
use std::cmp::Ordering;
use std::fmt;

#[derive(PartialEq, Eq, Clone, Copy, serde::Deserialize, serde::Serialize)]
pub struct PublicKey(ext::PublicKey);
pub struct SecretKey(ext::SecretKey);
pub struct Keypair {
    pub secret: SecretKey,
    pub public: PublicKey,
}
#[derive(Clone)]
pub struct Signature(ext::Signature);

pub type SignatureError = ext::SignatureError;

impl Keypair {
    pub fn generate() -> Self {
        // TODO: Not using Keypair::generate because `ed25519_dalek` uses an incompatible version
        // of the `rand` dependency.
        // https://stackoverflow.com/questions/65562447/the-trait-rand-corecryptorng-is-not-implemented-for-osrng
        // https://github.com/dalek-cryptography/ed25519-dalek/issues/162
        let mut bytes = [0u8; ext::SECRET_KEY_LENGTH];
        OsRng {}.fill(&mut bytes[..]);
        let sk = ext::SecretKey::from_bytes(&bytes).unwrap();
        let pk = (&sk).into();
        Keypair {
            secret: SecretKey(sk),
            public: PublicKey(pk),
        }
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        let expanded: ext::ExpandedSecretKey = (&self.secret.0).into();
        Signature(expanded.sign(msg, &self.public.0))
    }
}

impl PublicKey {
    pub fn verify(&self, msg: &[u8], signature: &Signature) -> bool {
        self.0.verify(msg, &signature.0).is_ok()
    }
}

impl PartialOrd for PublicKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.0.as_bytes().partial_cmp(other.0.as_bytes())
    }
}

impl Ord for PublicKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.as_bytes().cmp(other.0.as_bytes())
    }
}

impl fmt::LowerHex for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, self.0.as_bytes())
    }
}

// https://github.com/dalek-cryptography/ed25519-dalek/issues/183
impl Hash for PublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_bytes().hash(state);
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl TryFrom<&'_ [u8]> for PublicKey {
    type Error = ext::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let pk = ext::PublicKey::from_bytes(bytes)?;
        Ok(Self(pk))
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:8x}", self)
    }
}

// TODO: Temporarily enabling for non tests as well.
//#[cfg(test)]
impl rand::distributions::Distribution<PublicKey> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> PublicKey {
        let bytes: [u8; ext::PUBLIC_KEY_LENGTH] = self.sample(rng);
        let pk = ext::PublicKey::from_bytes(&bytes).unwrap();
        PublicKey(pk)
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(PublicKey);

impl AsRef<[u8]> for Signature {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl TryFrom<&'_ [u8]> for Signature {
    type Error = ext::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        let sig = ext::Signature::from_bytes(bytes)?;
        Ok(Signature(sig))
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(Signature);

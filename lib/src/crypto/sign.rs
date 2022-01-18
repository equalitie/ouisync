use crate::{
    crypto::{Digest, Hashable},
    format,
};
use ed25519_dalek as ext;
use ed25519_dalek::Verifier;
use rand::{rngs::OsRng, CryptoRng, Rng};
use serde;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash as StdHash, Hasher},
    str::FromStr,
};
use zeroize::Zeroize;

#[derive(PartialEq, Eq, Clone, Copy, Deserialize, Serialize)]
#[repr(transparent)]
pub struct PublicKey(ext::PublicKey);

#[repr(transparent)]
pub struct SecretKey(ext::SecretKey);

pub struct Keypair {
    pub secret: SecretKey,
    pub public: PublicKey,
}
#[derive(PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Signature(ext::Signature);

pub type SignatureError = ext::SignatureError;

impl Keypair {
    pub fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        let secret = SecretKey::generate(rng);
        let public = PublicKey::from(&secret);

        Self { secret, public }
    }

    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.secret.sign(msg, &self.public)
    }
}

impl From<SecretKey> for Keypair {
    fn from(secret: SecretKey) -> Self {
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }
}

impl PublicKey {
    pub const SIZE: usize = ext::PUBLIC_KEY_LENGTH;

    // // TODO: Temporarily enabling for non tests as well.
    // //#[cfg(test)]
    pub fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        (&SecretKey::generate(rng)).into()
    }

    #[cfg(test)]
    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }

    pub fn verify(&self, msg: &[u8], signature: &Signature) -> bool {
        self.0.verify(msg, &signature.0).is_ok()
    }

    pub fn starts_with(&self, needle: &[u8]) -> bool {
        self.0.as_ref().starts_with(needle)
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
#[allow(clippy::derive_hash_xor_eq)]
impl StdHash for PublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        StdHash::hash(self.0.as_bytes(), state);
    }
}

impl Hashable for PublicKey {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.0.as_bytes().update_hash(state)
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
        Ok(Self(ext::PublicKey::from_bytes(bytes)?))
    }
}

impl From<[u8; Self::SIZE]> for PublicKey {
    fn from(bytes: [u8; Self::SIZE]) -> Self {
        Self::try_from(bytes.as_ref()).unwrap()
    }
}

impl From<PublicKey> for [u8; PublicKey::SIZE] {
    fn from(key: PublicKey) -> Self {
        key.0.to_bytes()
    }
}

impl<'a> From<&'a SecretKey> for PublicKey {
    fn from(sk: &'a SecretKey) -> Self {
        Self((&sk.0).into())
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

impl FromStr for PublicKey {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; ext::PUBLIC_KEY_LENGTH];
        hex::decode_to_slice(s, &mut bytes)?;

        Ok(Self(ext::PublicKey::from_bytes(&bytes).unwrap()))
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(PublicKey);

impl SecretKey {
    pub const SIZE: usize = ext::SECRET_KEY_LENGTH;

    pub fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        // TODO: Not using SecretKey::generate because `ed25519_dalek` uses an incompatible version
        // of the `rand` dependency.
        // https://stackoverflow.com/questions/65562447/the-trait-rand-corecryptorng-is-not-implemented-for-osrng
        // https://github.com/dalek-cryptography/ed25519-dalek/issues/162

        let mut bytes = [0u8; ext::SECRET_KEY_LENGTH];
        rng.fill(&mut bytes[..]);

        // The unwrap is ok because `bytes` has the correct length.
        let sk = ext::SecretKey::from_bytes(&bytes).unwrap();

        bytes.zeroize();

        Self(sk)
    }

    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }

    pub fn sign(&self, msg: &[u8], public_key: &PublicKey) -> Signature {
        let expanded: ext::ExpandedSecretKey = (&self.0).into();
        Signature(expanded.sign(msg, &public_key.0))
    }
}

impl TryFrom<&[u8]> for SecretKey {
    type Error = ext::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(ext::SecretKey::from_bytes(bytes)?))
    }
}

impl AsRef<[u8; Self::SIZE]> for SecretKey {
    fn as_ref(&self) -> &[u8; Self::SIZE] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SecretKey(****)")
    }
}

impl Signature {
    pub const SIZE: usize = ext::SIGNATURE_LENGTH;
}

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

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Signature(_)")
    }
}

derive_sqlx_traits_for_byte_array_wrapper!(Signature);

use crate::{
    crypto::{Digest, Hashable},
    format,
};
use ed25519_dalek as ext;
use ed25519_dalek::Verifier;
use rand::{rngs::OsRng, CryptoRng, Rng};
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

#[derive(Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
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
        Some(self.cmp(other))
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
#[allow(clippy::derived_hash_with_manual_eq)]
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
        write!(f, "{self:x}")
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:8x}")
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

#[cfg(test)]
mod test_utils {
    use super::{PublicKey, SecretKey};
    use proptest::{
        arbitrary::{any, Arbitrary},
        array::UniformArrayStrategy,
        num,
        strategy::{Map, NoShrink, Strategy},
    };

    impl Arbitrary for PublicKey {
        type Parameters = ();
        type Strategy = Map<
            NoShrink<UniformArrayStrategy<num::u8::Any, [u8; SecretKey::SIZE]>>,
            fn([u8; SecretKey::SIZE]) -> Self,
        >;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            any::<[u8; SecretKey::SIZE]>()
                .no_shrink()
                .prop_map(|array| (&SecretKey::try_from(&array[..]).unwrap()).into())
        }
    }
}

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

    pub(crate) fn as_array(&self) -> &[u8; Self::SIZE] {
        self.0.as_bytes()
    }
}

impl TryFrom<&[u8]> for SecretKey {
    type Error = ext::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(ext::SecretKey::from_bytes(bytes)?))
    }
}

impl AsRef<[u8]> for SecretKey {
    fn as_ref(&self) -> &[u8] {
        &self.as_array()[..]
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

impl From<[u8; Self::SIZE]> for Signature {
    fn from(bytes: [u8; Self::SIZE]) -> Self {
        Self::try_from(bytes.as_ref()).unwrap()
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

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use rand::{distributions::Standard, rngs::StdRng, SeedableRng};

    // This test asserts that signatures from the same keys and input are identical between
    // different versions.
    #[test]
    fn compatibility() {
        let mut rng = StdRng::seed_from_u64(0);
        let keypair = Keypair::generate(&mut rng);

        assert_snapshot!(
            dump_signature(&keypair.sign(b"")),
            @"51ad17bc6bfbeeddd86c2a328d7a9b37197453244f4470a446ac9516acb4f243add7f93a5a6ba44bd21b9ed45c830dbbe28e2c40f7819d4c42c45b844258140a"
        );

        assert_snapshot!(
            dump_signature(&keypair.sign(b"hello world")),
            @"bcbd9b3aee0031f9616ed873106f2a0a136572fb5182c71e8d56c1308098c7c687367608e99bb64ace8de09544e8d87dc46e0cdaa7d188ee78bfbfb7d754a703"
        );

        assert_snapshot!(
            dump_signature(&keypair.sign(&rng.sample_iter(Standard).take(32).collect::<Vec<_>>())),
            @"3230b7f98529273c71f8af92b1581d290bf424fd7bd5015399c6213cbc461ca79ff932a7fcbb5e19d2ef6efa8ed9b833b6d17431793facf1b810c3b579570d0d"
        );
    }

    fn dump_signature(signature: &Signature) -> String {
        hex::encode(signature.as_ref())
    }
}

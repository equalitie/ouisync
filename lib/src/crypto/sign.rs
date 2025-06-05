use crate::crypto::{Digest, Hashable};
use ed25519_dalek::{self as ext, Signer, Verifier};
use rand::{rngs::OsRng, CryptoRng, Rng};
use serde::{Deserialize, Serialize};
use sqlx::{
    sqlite::{SqliteArgumentValue, SqliteTypeInfo, SqliteValueRef},
    Sqlite,
};
use std::{cmp::Ordering, fmt, str::FromStr};
use thiserror::Error;

#[derive(Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Keypair(ext::SigningKey);

impl Keypair {
    pub const SECRET_KEY_SIZE: usize = ext::SECRET_KEY_LENGTH;

    pub fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        Self(ext::SigningKey::generate(rng))
    }

    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }

    pub fn to_bytes(&self) -> [u8; Self::SECRET_KEY_SIZE] {
        self.0.to_bytes()
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key())
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        Signature(self.0.sign(msg))
    }
}

impl From<&'_ [u8; Self::SECRET_KEY_SIZE]> for Keypair {
    fn from(bytes: &'_ [u8; Self::SECRET_KEY_SIZE]) -> Self {
        Self(ext::SigningKey::from(bytes))
    }
}

impl TryFrom<&'_ [u8]> for Keypair {
    type Error = SignatureError;

    fn try_from(bytes: &'_ [u8]) -> Result<Self, Self::Error> {
        Ok(Self(ext::SigningKey::try_from(bytes)?))
    }
}

impl fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Keypair")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Copy, Deserialize, Serialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct PublicKey(ext::VerifyingKey);

impl PublicKey {
    pub const SIZE: usize = ext::PUBLIC_KEY_LENGTH;

    #[cfg(test)]
    pub fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        Keypair::generate(rng).public_key()
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
        hex_fmt::HexFmt(self.0.as_bytes()).fmt(f)
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
        Ok(Self(bytes.try_into()?))
    }
}

impl From<PublicKey> for [u8; PublicKey::SIZE] {
    fn from(key: PublicKey) -> Self {
        key.0.to_bytes()
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:x}")
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:<8x}")
    }
}

impl FromStr for PublicKey {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; Self::SIZE];
        hex::decode_to_slice(s, &mut bytes).map_err(|_| ParseError)?;
        Self::try_from(&bytes[..]).map_err(|_| ParseError)
    }
}

#[derive(Debug, Error)]
#[error("failed to parse public key")]
pub struct ParseError;

derive_sqlx_traits_for_byte_array_wrapper!(PublicKey);

#[cfg(test)]
mod test_utils {
    use super::{Keypair, PublicKey};
    use proptest::{
        arbitrary::{any, Arbitrary},
        array::UniformArrayStrategy,
        num,
        strategy::{Map, NoShrink, Strategy},
    };

    impl Arbitrary for Keypair {
        type Parameters = ();
        type Strategy = Map<
            NoShrink<UniformArrayStrategy<num::u8::Any, [u8; Keypair::SECRET_KEY_SIZE]>>,
            fn([u8; Keypair::SECRET_KEY_SIZE]) -> Self,
        >;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            any::<[u8; Keypair::SECRET_KEY_SIZE]>()
                .no_shrink()
                .prop_map(|array| Keypair::from(&array))
        }
    }

    impl Arbitrary for PublicKey {
        type Parameters = ();
        type Strategy = Map<<Keypair as Arbitrary>::Strategy, fn(Keypair) -> Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            any::<Keypair>().prop_map(|keypair| keypair.public_key())
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Signature(ext::Signature);

impl Signature {
    pub const SIZE: usize = ext::SIGNATURE_LENGTH;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        self.0.to_bytes()
    }
}

impl From<&'_ [u8; Self::SIZE]> for Signature {
    fn from(bytes: &'_ [u8; Self::SIZE]) -> Self {
        Self(ext::Signature::from(bytes))
    }
}

impl TryFrom<&'_ [u8]> for Signature {
    type Error = ext::SignatureError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Signature(ext::Signature::try_from(bytes)?))
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Signature(_)")
    }
}

impl sqlx::Type<Sqlite> for Signature {
    fn type_info() -> SqliteTypeInfo {
        <&[u8] as sqlx::Type<Sqlite>>::type_info()
    }
}

impl<'q> sqlx::Encode<'q, Sqlite> for &'q Signature {
    fn encode_by_ref(
        &self,
        args: &mut Vec<SqliteArgumentValue<'q>>,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        // It seems there is no way to avoid the allocation here because sqlx doesn't implement
        // `Encode` for arrays.
        sqlx::Encode::<Sqlite>::encode(self.to_bytes().to_vec(), args)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Signature {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let slice = <&[u8] as sqlx::Decode<Sqlite>>::decode(value)?;
        Ok(slice.try_into()?)
    }
}

pub type SignatureError = ext::SignatureError;

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, SeedableRng};

    // This test asserts that signatures from the same keys and input are identical between
    // different versions.
    #[test]
    fn compatibility() {
        let mut rng = StdRng::seed_from_u64(0);
        let keypair = Keypair::generate(&mut rng);

        assert_eq!(
            dump_signature(&keypair.sign(b"")),
            "51ad17bc6bfbeeddd86c2a328d7a9b37197453244f4470a446ac9516acb4f243add7f93a5a6ba44bd21b9ed45c830dbbe28e2c40f7819d4c42c45b844258140a"
        );

        assert_eq!(
            dump_signature(&keypair.sign(b"hello world")),
            "bcbd9b3aee0031f9616ed873106f2a0a136572fb5182c71e8d56c1308098c7c687367608e99bb64ace8de09544e8d87dc46e0cdaa7d188ee78bfbfb7d754a703"
        );

        assert_eq!(
            dump_signature(&keypair.sign(&rng.r#gen::<[u8; 32]>())),
            "3230b7f98529273c71f8af92b1581d290bf424fd7bd5015399c6213cbc461ca79ff932a7fcbb5e19d2ef6efa8ed9b833b6d17431793facf1b810c3b579570d0d"
        );
    }

    fn dump_signature(signature: &Signature) -> String {
        hex::encode(signature.to_bytes())
    }
}

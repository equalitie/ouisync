//! Encryption / Decryption utilities.

use super::{hash::Digest, password::PasswordSalt};
use chacha20::{
    cipher::{KeyIvInit, StreamCipher},
    ChaCha20,
};
use generic_array::{sequence::GenericSequence, typenum::Unsigned};
use hex;
use ouisync_macros::api;
use rand::{rngs::OsRng, CryptoRng, Rng};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, sync::Arc};
use subtle::ConstantTimeEq;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

/// Nonce
pub(crate) type Nonce = [u8; NONCE_SIZE];
pub(crate) const NONCE_SIZE: usize =
    <<chacha20::Nonce as GenericSequence<_>>::Length as Unsigned>::USIZE;

/// Symmetric encryption/decryption secret key.
///
/// Note: this implementation tries to prevent certain types of attacks by making sure the
/// underlying sensitive key material is always stored at most in one place. This is achieved by
/// putting it on the heap which means it is not moved when the key itself is moved which could
/// otherwise leave a copy of the data in memory. Additionally, the data is behind a `Arc` which
/// means the key can be cheaply cloned without actually cloning the data. Finally, the data is
/// scrambled (overwritten with zeros) when the key is dropped to make sure it does not stay in
/// the memory past its lifetime.
#[derive(Clone)]
#[api(repr(Bytes), secret)]
pub struct SecretKey(Arc<Zeroizing<[u8; Self::SIZE]>>);

impl SecretKey {
    /// Size of the key in bytes.
    pub const SIZE: usize = <<chacha20::Key as GenericSequence<_>>::Length as Unsigned>::USIZE;

    /// Parse secret key from hexadecimal string of size 2*SIZE.
    pub fn parse_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let mut bytes = [0; Self::SIZE];
        hex::decode_to_slice(hex_str, &mut bytes)?;

        let mut key = Self::zero();
        key.as_mut().copy_from_slice(&bytes);

        bytes.zeroize();

        Ok(key)
    }

    /// Generate a random secret key using the given cryptographically secure random number
    /// generator.
    ///
    /// Note: this is purposefully not implemented as `impl Distribution<SecretKey> for Standard`
    /// to enforce the additional `CryptoRng` bound.
    pub fn generate<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        // Create all-zero array initially, then fill it with random bytes in place to avoid moving
        // the array which could leave the sensitive data in memory.
        let mut key = Self::zero();
        rng.fill(key.as_mut());
        key
    }

    /// Generate a random secret key using the default RNG.
    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }

    /// Derive a secret key from another secret key and a nonce.
    pub fn derive_from_key(master_key: &[u8; Self::SIZE], nonce: &[u8]) -> Self {
        let mut sub_key = Self::zero();

        let mut hasher = blake3::Hasher::new_keyed(master_key);
        hasher.update(nonce);
        hasher.finalize_into(sub_key.as_mut().into());

        sub_key
    }

    /// Derive a secret key from user's password and salt.
    pub fn derive_from_password(user_password: &str, salt: &PasswordSalt) -> Self {
        use argon2::{Algorithm, Argon2, ParamsBuilder, Version};

        let mut result = Self::zero();

        Argon2::new(
            Algorithm::default(),
            Version::default(),
            // Using explicit params to preserve the output from v0.4 of argon2. See
            // https://github.com/equalitie/ouisync/issues/144 for details.
            ParamsBuilder::new().m_cost(4096).t_cost(3).build().unwrap(),
        )
        .hash_password_into(user_password.as_ref(), salt.as_ref(), result.as_mut())
        // Note: we control the output and salt size. And the only other check that this
        // function does is whether the password isn't too long, but that would have to be more
        // than 0xffffffff so the `.expect` shouldn't be an issue.
        .expect("failed to hash password");

        result
    }

    // TODO: the following two functions have identical implementations. Consider replacing them
    // with a single function (what should it be called?).

    /// Encrypt a message in place without using Authenticated Encryption with Associated Data
    pub(crate) fn encrypt_no_aead(&self, nonce: &Nonce, buffer: &mut [u8]) {
        let mut cipher = ChaCha20::new(self.as_ref().into(), nonce.into());
        cipher.apply_keystream(buffer)
    }

    /// Decrypt a message in place without using Authenticated Encryption with Associated Data.
    pub(crate) fn decrypt_no_aead(&self, nonce: &Nonce, buffer: &mut [u8]) {
        let mut cipher = ChaCha20::new(self.as_ref().into(), nonce.into());
        cipher.apply_keystream(buffer)
    }

    /// Note this method is somewhat dangerous because if used carelessly the underlying sensitive data
    /// can be copied or revealed.
    pub fn as_array(&self) -> &[u8; Self::SIZE] {
        &self.0
    }

    // Use this only for initialization.
    fn zero() -> Self {
        Self(Arc::new(Zeroizing::new([0; Self::SIZE])))
    }

    // Use this only for initialization. Panics if this key has more than one clone.
    fn as_mut(&mut self) -> &mut [u8] {
        &mut **Arc::get_mut(&mut self.0).unwrap()
    }
}

impl TryFrom<&[u8]> for SecretKey {
    type Error = SecretKeyLengthError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        if slice.len() >= Self::SIZE {
            let mut key = Self::zero();
            key.as_mut().copy_from_slice(slice);
            Ok(key)
        } else {
            Err(SecretKeyLengthError)
        }
    }
}

/// Note this trait is somewhat dangerous because if used carelessly the underlying sensitive data
/// can be copied or revealed.
impl AsRef<[u8]> for SecretKey {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

/// Note this impl uses constant-time operations (using [subtle](https://crates.io/crates/subtle))
/// and so provides protection against software side-channel attacks.
impl PartialEq for SecretKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_array().ct_eq(other.as_array()).into()
    }
}

impl Eq for SecretKey {}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "****")
    }
}

impl Serialize for SecretKey {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_bytes::Bytes::new(self.as_ref()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for SecretKey {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: &serde_bytes::Bytes = Deserialize::deserialize(d)?;

        if bytes.len() != Self::SIZE {
            return Err(D::Error::invalid_length(
                bytes.len(),
                &format!("{}", Self::SIZE).as_str(),
            ));
        }

        let mut key = Self::zero();
        key.as_mut().copy_from_slice(bytes);

        Ok(key)
    }
}

#[derive(Debug, Error)]
#[error("invalid secret key length")]
pub struct SecretKeyLengthError;

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn serialize_deserialize_bincode() {
        let orig = SecretKey::try_from(&b"abcdefghijklmnopqrstuvwxyz012345"[..]).unwrap();
        let expected_serialized_hex =
            "20000000000000006162636465666768696a6b6c6d6e6f707172737475767778797a303132333435";

        let serialized = bincode::serialize(&orig).unwrap();
        assert_eq!(hex::encode(&serialized), expected_serialized_hex);

        let deserialized: SecretKey = bincode::deserialize(&serialized).unwrap();
        assert_eq!(deserialized.as_ref(), orig.as_ref());
    }

    #[test]
    fn serialize_deserialize_msgpack() {
        let orig = SecretKey::try_from(&b"abcdefghijklmnopqrstuvwxyz012345"[..]).unwrap();
        let expected_serialized_hex =
            "c4206162636465666768696a6b6c6d6e6f707172737475767778797a303132333435";

        let serialized = rmp_serde::to_vec(&orig).unwrap();
        assert_eq!(hex::encode(&serialized), expected_serialized_hex);

        let deserialized: SecretKey = rmp_serde::from_slice(&serialized).unwrap();
        assert_eq!(deserialized.as_ref(), orig.as_ref());
    }

    #[test]
    fn derive_from_password_snapshot() {
        let test_vectors = [
            (
                "jxzBql3QHxENyynvh2SICH9ND",
                "a2849a63283cbaf0fdbceb1f6479b197",
                "c7ffa7d05f0898d71839cbd62a00a9616904d795c0372704c22bf76c363b371c",
            ),
            (
                "4oveW4y3uE7KNbT6Yr",
                "9410973ae328ad92916268128edb4710",
                "5796c8ff977611fa48009fa690cd4091928c65a5c8587babcd06bdb76f8a4793",
            ),
            (
                "3wQ3KFPW5L4hnaQZQeq",
                "7d49d2b38763a12b2bbdfa93275aff18",
                "b358fb33b5aacda99d59cd6b9c1c8e7e2162bea31addc22361220f05781e9736",
            ),
            (
                "DL3NuUTWnjMocBkFMUuP",
                "8c89c7108fff2095e18ddfef8986b118",
                "8ebf12a4306b97b4978cac088e80b76a2cf31ae19b3c9c746a53c6d4101ee3be",
            ),
            (
                "9wD4BW85Ji8GjS2XvxC2dLVgpMZGeS",
                "b2a44461cc0bebb325280ed9130a59bb",
                "f2847b060d5ffdcd8a1baf0264b12fc7f67e5ea61350defe612f7e9d1a1b29d3",
            ),
            (
                "HFaiwNNJuSULjQlWRSo",
                "46ece5c682cd598a65eabff63a3572df",
                "05b17ea51240753a7d7fc707830316017dbf108dda822752ed0a31dd02655ac3",
            ),
            (
                "pA4Zlnc7NQYej1nbQYWERDh1fH",
                "2e0151573fe9c69df29b830987990985",
                "186451be15369704049a1237c80306e0a0d6638a97a1e737b59043cc8578d374",
            ),
            (
                "XPrtoJzqdo03fArXYvLCbqxTckLhi",
                "2b852c5409d6c6813c49d1379cbbc1e9",
                "77a08ef20a56b62a89c8b66ea6a9a489b863b3fcf06d5a9edf663729ee0ea2cd",
            ),
            (
                "oWeBi4ghF",
                "277c27b1587751f2af2001be3712ef0d",
                "16e2b5b01852443672da19b47bacd31e3fb3cda972a9f52440a54ad811f7bcb8",
            ),
            (
                "9FvIzpn7tV0l7lADIsQDbAZ",
                "f864670399430d1671c31a2431183625",
                "20ce1c73cfed98a27b8d2b63cb5a06709a92d471188c7398e521c83ea51cb766",
            ),
            (
                "uEqxiIP533EbTK8MK5tEozAsn1nS",
                "69b52967216f8f3ff5a1fa73e5046315",
                "40e51fb3f0ecaf38abdcb018bdeb0935a484d9d365a0ce47ce277e72d68ea4ea",
            ),
            (
                "wMybpBIOzN5P",
                "bca1f48e60bad68798a828d3efd5258a",
                "833acd5273e323cff011aefd83ff64ac33d72d30d3cdd1b01a03dc16a4a6b1c6",
            ),
            (
                "YQ3Z8mGqp97XGJRV1LT",
                "b469f80e382214b5f157d1c7d36a2058",
                "8d1b82de3ed96198378a9a8f9fcffa5203f94e2c344092be4ecc89ccfa7f0173",
            ),
            (
                "otSkYO0uFASqZ",
                "10b6a29fa50416e276a0e79cbe66534e",
                "4d3226dc093cbbfa6247c26f317e784f9f3f975fe05b9abb84fd58996e603534",
            ),
            (
                "jWwusg8vdvIQCeC2y9",
                "956071ee6e80c856f20744a8e5d6ca27",
                "00eec6c9a868bae430f1f4588b7f2357bf5114fcbb65beb2e65e9e4202c7f75e",
            ),
            (
                "NBNh4UYsHlc",
                "f61877af4e7f8313ad8234302950b331",
                "5a5685bf7a635ea8d86967251ebbe029f2782d36f5e0296a9775076002ab34d7",
            ),
        ];

        for (password, salt, expected_secret_key) in test_vectors {
            let salt: [u8; PasswordSalt::SIZE] = hex::decode(salt).unwrap().try_into().unwrap();
            let salt = PasswordSalt::from(salt);

            let actual_secret_key = SecretKey::derive_from_password(password, &salt);
            let actual_secret_key = hex::encode(actual_secret_key.as_array());

            assert_eq!(actual_secret_key, expected_secret_key);
        }
    }
}

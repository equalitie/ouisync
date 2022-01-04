//! Encryption / Decryption utilities.

// reexport for convenience
pub use chacha20poly1305::aead;

use super::password::PasswordSalt;
use argon2::Argon2;
use chacha20poly1305::{
    aead::{AeadInPlace, NewAead},
    ChaCha20Poly1305,
};
use generic_array::{sequence::GenericSequence, typenum::Unsigned};
use hex;
use rand::{rngs::OsRng, CryptoRng, Rng};
use sha3::{
    digest::{Digest, FixedOutput},
    Sha3_256,
};
use std::{fmt, sync::Arc};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

/// Nonce
pub type Nonce = [u8; NONCE_SIZE];
pub const NONCE_SIZE: usize =
    <<chacha20poly1305::Nonce as GenericSequence<_>>::Length as Unsigned>::USIZE;

/// Authentication tag.
pub type AuthTag = chacha20poly1305::Tag;
pub const AUTH_TAG_SIZE: usize = <<AuthTag as GenericSequence<_>>::Length as Unsigned>::USIZE;

const SECRET_KEY_SIZE: usize =
    <<chacha20poly1305::Key as GenericSequence<_>>::Length as Unsigned>::USIZE;

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
pub struct SecretKey(Arc<Zeroizing<[u8; SECRET_KEY_SIZE]>>);

impl SecretKey {
    /// Size of the key in bytes.
    pub const SIZE: usize = SECRET_KEY_SIZE;

    /// Parse secret key from hexadecimal string of size 2*SIZE.
    pub fn parse_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let mut bytes = [0; Self::SIZE];
        hex::decode_to_slice(&hex_str, &mut bytes)?;

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
    pub fn derive_from_key(master_key: &[u8], nonce: &[u8]) -> Self {
        let mut sub_key = Self::zero();

        // TODO: verify this is actually secure!
        let mut hasher = Sha3_256::new();
        hasher.update(master_key);
        hasher.update(nonce);
        hasher.finalize_into(sub_key.as_mut().into());

        sub_key
    }

    /// Derive a secret key from user's password and salt.
    pub fn derive_from_password(user_password: &str, salt: &PasswordSalt) -> Self {
        let mut result = Self::zero();
        // Note: we controll the output and salt size. And the only other check that this function
        // does is whether the password isn't too long, but that would have to be more than
        // 0xffffffff so the `.expect` shouldn't be an issue.
        Argon2::default()
            .hash_password_into(user_password.as_ref(), salt, result.as_mut())
            .expect("failed to hash password");
        result
    }

    /// Shortcut for `SecretKey::derive_from_key(self.as_ref(), nonce)` for convenience.
    pub fn derive_subkey(&self, nonce: &[u8]) -> Self {
        Self::derive_from_key(self.as_ref(), nonce)
    }

    /// Encrypt a message in place using Authenticated Encryption with Associated Data.
    pub fn encrypt(
        &self,
        nonce: Nonce,
        aad: &[u8],
        buffer: &mut [u8],
    ) -> Result<AuthTag, aead::Error> {
        let cipher = ChaCha20Poly1305::new(self.as_ref().into());
        cipher.encrypt_in_place_detached(&nonce.into(), aad, buffer)
    }

    /// Decrypt a message in place using Authenticated Encryption with Associated Data.
    pub fn decrypt(
        &self,
        nonce: Nonce,
        aad: &[u8],
        buffer: &mut [u8],
        auth_tag: &AuthTag,
    ) -> Result<(), aead::Error> {
        let cipher = ChaCha20Poly1305::new(self.as_ref().into());
        cipher.decrypt_in_place_detached(&nonce.into(), aad, buffer, auth_tag)
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
        &**self.0
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SecretKey(****)")
    }
}

#[derive(Debug, Error)]
#[error("invalid secret key length")]
pub struct SecretKeyLengthError;

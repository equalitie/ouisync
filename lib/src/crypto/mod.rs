mod hash;
mod password;
pub mod secret_key;
pub mod sign;

pub use self::{
    hash::{Hash, Hashable},
    password::Password,
    secret_key::{PasswordSalt, SecretKey, SecretKeyLengthError},
};
pub use chacha20poly1305::aead;

use chacha20poly1305::aead::{AeadInPlace, NewAead};
use chacha20poly1305::ChaCha20Poly1305;
use generic_array::{sequence::GenericSequence, typenum::Unsigned};

/// Nonce size
pub const NONCE_SIZE: usize =
    <<chacha20poly1305::Nonce as GenericSequence<_>>::Length as Unsigned>::USIZE;

/// Nonce
pub type Nonce = [u8; NONCE_SIZE];

/// Authentication tag.
pub type AuthTag = chacha20poly1305::Tag;

pub const AUTH_TAG_SIZE: usize = <<AuthTag as GenericSequence<_>>::Length as Unsigned>::USIZE;

/// Encryptor/decryptor
#[derive(Clone)]
pub enum Cryptor {
    /// Cryptography using the `ChaCha20Poly1305` cipher.
    ChaCha20Poly1305(SecretKey),
    /// "null" cryptography that doesn't actually encrypt/decrypt. To be used when encyption is not
    /// needed.
    Null,
}

impl Cryptor {
    pub fn encrypt(
        &self,
        nonce: Nonce,
        aad: &[u8],
        buffer: &mut [u8],
    ) -> Result<AuthTag, aead::Error> {
        match self {
            Self::ChaCha20Poly1305(key) => {
                let cipher = ChaCha20Poly1305::new(key.as_array());
                cipher.encrypt_in_place_detached(&nonce.into(), aad, buffer)
            }
            Self::Null => Ok(AuthTag::default()),
        }
    }

    pub fn decrypt(
        &self,
        nonce: Nonce,
        aad: &[u8],
        buffer: &mut [u8],
        auth_tag: &AuthTag,
    ) -> Result<(), aead::Error> {
        match self {
            Self::ChaCha20Poly1305(key) => {
                let cipher = ChaCha20Poly1305::new(key.as_array());
                cipher.decrypt_in_place_detached(&nonce.into(), aad, buffer, auth_tag)
            }
            Self::Null => Ok(()),
        }
    }

    pub fn derive_subkey(&self, nonce: &[u8]) -> Self {
        match self {
            Self::ChaCha20Poly1305(key) => {
                Self::ChaCha20Poly1305(SecretKey::derive_from_key(key.as_array().as_ref(), nonce))
            }
            Self::Null => Self::Null,
        }
    }
}

pub mod cipher;
mod hash;
mod password;
pub mod sign;

pub(crate) use self::password::PasswordSalt;
pub use self::{
    hash::{Hash, Hashable},
    password::Password,
};

use self::cipher::{aead, AuthTag, Nonce, SecretKey};
use chacha20poly1305::aead::{AeadInPlace, NewAead};
use chacha20poly1305::ChaCha20Poly1305;

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
                let cipher = ChaCha20Poly1305::new(key.as_ref().into());
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
                let cipher = ChaCha20Poly1305::new(key.as_ref().into());
                cipher.decrypt_in_place_detached(&nonce.into(), aad, buffer, auth_tag)
            }
            Self::Null => Ok(()),
        }
    }

    pub fn derive_subkey(&self, nonce: &[u8]) -> Self {
        match self {
            Self::ChaCha20Poly1305(key) => {
                Self::ChaCha20Poly1305(SecretKey::derive_from_key(key.as_ref(), nonce))
            }
            Self::Null => Self::Null,
        }
    }
}

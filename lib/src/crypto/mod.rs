mod hash;
mod nonce;
mod secret_key;

pub use self::{
    hash::{Hash, Hashable},
    nonce::{Nonce, NoncePrefix, NonceSequence},
    secret_key::SecretKey,
};
/// Re-export the aead module for convenience.
pub use chacha20poly1305::aead;

use self::aead::{AeadInPlace, NewAead};
use chacha20poly1305::ChaCha20Poly1305;

/// Authentication tag.
pub type AuthTag = chacha20poly1305::Tag;

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
        nonce: &Nonce,
        aad: &[u8],
        buffer: &mut [u8],
    ) -> Result<AuthTag, aead::Error> {
        match self {
            Self::ChaCha20Poly1305(key) => {
                let cipher = ChaCha20Poly1305::new(key.as_array());
                cipher.encrypt_in_place_detached(nonce, aad, buffer)
            }
            Self::Null => Ok(AuthTag::default()),
        }
    }

    pub fn decrypt(
        &self,
        nonce: &Nonce,
        aad: &[u8],
        buffer: &mut [u8],
        auth_tag: &AuthTag,
    ) -> Result<(), aead::Error> {
        match self {
            Self::ChaCha20Poly1305(key) => {
                let cipher = ChaCha20Poly1305::new(key.as_array());
                cipher.decrypt_in_place_detached(nonce, aad, buffer, auth_tag)
            }
            Self::Null => Ok(()),
        }
    }
}

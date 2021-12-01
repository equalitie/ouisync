mod hash;
mod secret_key;

pub use self::{
    hash::{Hash, Hashable},
    secret_key::SecretKey,
};
/// Re-export the aead module for convenience.
pub use chacha20poly1305::aead;

use self::aead::{AeadInPlace, NewAead};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use generic_array::{sequence::GenericSequence, typenum::Unsigned};

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
        nonce: u64,
        aad: &[u8],
        buffer: &mut [u8],
    ) -> Result<AuthTag, aead::Error> {
        match self {
            Self::ChaCha20Poly1305(key) => {
                let cipher = ChaCha20Poly1305::new(key.as_array());
                let nonce = make_nonce(nonce);
                cipher.encrypt_in_place_detached(&nonce, aad, buffer)
            }
            Self::Null => Ok(AuthTag::default()),
        }
    }

    pub fn decrypt(
        &self,
        nonce: u64,
        aad: &[u8],
        buffer: &mut [u8],
        auth_tag: &AuthTag,
    ) -> Result<(), aead::Error> {
        match self {
            Self::ChaCha20Poly1305(key) => {
                let cipher = ChaCha20Poly1305::new(key.as_array());
                let nonce = make_nonce(nonce);
                cipher.decrypt_in_place_detached(&nonce, aad, buffer, auth_tag)
            }
            Self::Null => Ok(()),
        }
    }

    pub fn derive_subkey(&self, nonce: &[u8]) -> Self {
        match self {
            Self::ChaCha20Poly1305(key) => {
                Self::ChaCha20Poly1305(SecretKey::derive_from_key(key, nonce))
            }
            Self::Null => Self::Null,
        }
    }
}

fn make_nonce(counter: u64) -> Nonce {
    const LEN: usize = <<Nonce as GenericSequence<_>>::Length as Unsigned>::USIZE;

    let mut nonce = Nonce::default();
    nonce[LEN - 8..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

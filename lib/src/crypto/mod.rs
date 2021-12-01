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

pub(crate) fn encrypt(
    key: &SecretKey,
    nonce: u64,
    aad: &[u8],
    buffer: &mut [u8],
) -> Result<AuthTag, aead::Error> {
    ChaCha20Poly1305::new(key.as_array()).encrypt_in_place_detached(&make_nonce(nonce), aad, buffer)
}

pub(crate) fn decrypt(
    key: &SecretKey,
    nonce: u64,
    aad: &[u8],
    buffer: &mut [u8],
    auth_tag: &AuthTag,
) -> Result<(), aead::Error> {
    ChaCha20Poly1305::new(key.as_array()).decrypt_in_place_detached(
        &make_nonce(nonce),
        aad,
        buffer,
        auth_tag,
    )
}

fn make_nonce(counter: u64) -> Nonce {
    const LEN: usize = <<Nonce as GenericSequence<_>>::Length as Unsigned>::USIZE;

    let mut nonce = Nonce::default();
    nonce[LEN - 8..].copy_from_slice(&counter.to_le_bytes());
    nonce
}

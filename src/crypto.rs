/// Re-export the aead module for convenience.
pub use chacha20poly1305::aead;

use crate::format;
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::{CryptoRng, Rng};
use sha3::{
    digest::{generic_array::GenericArray, Digest},
    Sha3_256,
};
use std::{fmt, sync::Arc};
use zeroize::Zeroize;

/// Wrapper for a 256-bit hash digest, for convenience. Also implements friendly formatting.
#[derive(Copy, Clone, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct Hash(HashInner);

impl From<HashInner> for Hash {
    fn from(inner: HashInner) -> Self {
        Hash(inner)
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:8x}", self)
    }
}

impl fmt::LowerHex for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        format::hex(f, &self.0)
    }
}

type HashInner = GenericArray<u8, <Sha3_256 as Digest>::OutputSize>;

/// Cipher for symmetric encryption.
pub type Cipher = ChaCha20Poly1305;

/// Authentication tag.
pub type AuthTag = chacha20poly1305::Tag;

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
pub struct SecretKey(Arc<SecretKeyInner>);

impl SecretKey {
    /// Generate a random secret key using the given cryptographically secure random number
    /// generator.
    ///
    /// Note: this is purposefully not implement as `impl Distribution<SecretKey> for Standard` to
    /// enforce the additional `CryptoRng` bound.
    pub fn generate<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        // We can't mutate the data when it's in `Arc` so we need to create it outside of it first,
        // fill it with the random data and only then move it to the `Arc`. A seemingly more
        // straightforward way to do this would be to create the data on the stack first, but that
        // would create two copies of the key in memory. To avoid that, we create it in a `Box`
        // first and then convert it to `Arc`.
        let mut inner = Box::new(SecretKeyInner(chacha20poly1305::Key::default()));
        rng.fill(inner.0.as_mut_slice());
        Self(Arc::from(inner))
    }

    /// Generate a random secret key using the default RNG.
    pub fn random() -> Self {
        Self::generate(&mut rand::thread_rng())
    }

    // TODO: generate with KDF

    /// Returns reference to the underlying array.
    ///
    /// Note this function is somewhat dangerous because if used carelessly the underlying
    /// sensitive data can be copied or revealed.
    pub fn as_array(&self) -> &chacha20poly1305::Key {
        &self.0 .0
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SecretKey(****)")
    }
}

struct SecretKeyInner(chacha20poly1305::Key);

impl Drop for SecretKeyInner {
    fn drop(&mut self) {
        self.0.as_mut_slice().zeroize()
    }
}

const NONCE_PREFIX_SIZE: usize = 8;

pub struct NonceSequence {
    prefix: [u8; NONCE_PREFIX_SIZE],
}

impl NonceSequence {
    pub fn random() -> Self {
        let mut prefix = [0; NONCE_PREFIX_SIZE];
        rand::thread_rng().fill(&mut prefix[..]);

        Self { prefix }
    }

    /// Gets the shared prefix of each nonce of this sequence.
    pub fn prefix(&self) -> &[u8; NONCE_PREFIX_SIZE] {
        &self.prefix
    }

    /// Gets the `index`-th nonce of this sequence.
    pub fn get(&self, index: u32) -> Nonce {
        let mut nonce = Nonce::default();
        nonce[..NONCE_PREFIX_SIZE].copy_from_slice(&self.prefix);
        nonce[NONCE_PREFIX_SIZE..].copy_from_slice(&index.to_le_bytes());
        nonce
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_sequence() {
        let nonce = Nonce::default();
        assert_eq!(nonce.len(), NONCE_PREFIX_SIZE + 4);
    }
}

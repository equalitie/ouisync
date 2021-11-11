use rand::{CryptoRng, Rng};
use std::{fmt, sync::Arc};
use zeroize::Zeroize;

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
pub struct SecretKey(Arc<Inner>);

impl SecretKey {
    /// Generate a random secret key using the given cryptographically secure random number
    /// generator.
    ///
    /// Note: this is purposefully not implement as `impl Distribution<SecretKey> for Standard` to
    /// enforce the additional `CryptoRng` bound.
    pub fn generate<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        let mut array = Array::default();
        rng.fill(array.as_mut_slice());

        let key = Self(Arc::from(Inner(array)));

        // scramble the original location to prevent leaving the sensitive data in the memory.
        array.as_mut_slice().zeroize();

        key
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
    pub fn as_array(&self) -> &Array {
        &self.0 .0
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SecretKey(****)")
    }
}

struct Inner(Array);

impl Drop for Inner {
    fn drop(&mut self) {
        self.0.as_mut_slice().zeroize()
    }
}

type Array = chacha20poly1305::Key;

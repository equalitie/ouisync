use rand::{CryptoRng, Rng};
use sha3::{
    digest::{Digest, FixedOutput},
    Sha3_256,
};
use generic_array::{sequence::GenericSequence, typenum::Unsigned};
use hex;

// TODO: Using scrypt because argon2 v0.3.2 did not compile.
use scrypt::{
    password_hash::{
        self,
        rand_core::{OsRng, RngCore},
    },
    scrypt, Params,
};
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

const SCRYPT_SALT_LEN: usize = password_hash::Salt::RECOMMENDED_LENGTH;
pub type PasswordSalt = [u8; SCRYPT_SALT_LEN];

impl SecretKey {
    /// Size of the key in bytes.
    pub const SIZE: usize = <<Array as GenericSequence<_>>::Length as Unsigned>::USIZE;

    /// Load secret key from byte array of size SIZE.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(Arc::new(Inner(*Array::from_slice(bytes))))
    }

    /// Load secret key from hexadecimal string of size 2*SIZE.
    pub fn from_hex(hex_str: &str) -> Self {
        let mut bytes = hex::decode(&hex_str).expect("failed to decode the secret key from hex");
        let key = Self::from_bytes(&bytes);
        bytes.zeroize();
        key
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
        rng.fill(key.as_array_mut().as_mut_slice());
        key
    }

    /// Generate a random secret key using the default RNG.
    pub fn random() -> Self {
        Self::generate(&mut rand::thread_rng())
    }

    /// Derive a secret key from another secret key and a nonce.
    pub fn derive_from_key(master_key: &Self, nonce: &[u8]) -> Self {
        let mut sub_key = Self::zero();

        // TODO: verify this is actually secure!
        let mut hasher = Sha3_256::new();
        hasher.update(master_key.as_array());
        hasher.update(nonce);
        hasher.finalize_into(sub_key.as_array_mut());

        sub_key
    }

    /// Derive a secret key from user's password and salt.
    pub fn derive_from_password(user_password: &str, salt: &PasswordSalt) -> Self {
        let mut result = Self::zero();

        scrypt(
            user_password.as_bytes(),
            salt,
            &Params::recommended(),
            result.as_array_mut(),
        )
        .expect("failed to derive scrypt password");

        result
    }

    /// Generate random salt for use with the `derive_scrypt` function.
    pub fn generate_password_salt() -> PasswordSalt {
        let mut salt = [0u8; SCRYPT_SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        salt
    }

    /// Returns reference to the underlying array.
    ///
    /// Note this function is somewhat dangerous because if used carelessly the underlying
    /// sensitive data can be copied or revealed.
    pub fn as_array(&self) -> &Array {
        &self.0 .0
    }

    // Use this only for initialization.
    fn zero() -> Self {
        Self(Arc::new(Inner(Array::default())))
    }

    // Use this only for initialization. Panics if this key has more than one clone.
    fn as_array_mut(&mut self) -> &mut Array {
        &mut Arc::get_mut(&mut self.0).unwrap().0
    }
}

impl From<[u8; Self::SIZE]> for SecretKey {
    fn from(mut bytes: [u8; Self::SIZE]) -> Self {
        let mut key = Self::zero();
        key.as_array_mut().copy_from_slice(&bytes);
        bytes.zeroize();
        key
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

use crate::format;
use chacha20poly1305::{
    aead::stream::{DecryptorLE31, EncryptorLE31},
    ChaCha20Poly1305,
};
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

/// Stream encryptor.
pub type Encryptor = EncryptorLE31<ChaCha20Poly1305>;

/// Stream decryptor.
pub type Decryptor = DecryptorLE31<ChaCha20Poly1305>;

// TODO: crypto demonstration - to be deleted
#[test]
fn demo() {
    use chacha20poly1305::aead::{generic_array::GenericArray, Payload};

    let blocks_plain_original = vec![
        (b"foo".to_vec(), b"Lorem ipsum dolor sit amet".to_vec()),
        (b"bar".to_vec(), b"consectetur adipiscing elit".to_vec()),
        (b"baz".to_vec(), b"sed do eiusmod tempor".to_vec()),
    ];

    let key = SecretKey::random();

    // Nonce size is the nonce size of the underlying cipher minus nonce overhead of the
    // stream primitive. In this case it is 12 - 4 = 8
    let nonce = GenericArray::from_slice(b"a nonce.");

    // Encrypt

    let mut encryptor = Encryptor::new(key.as_array(), nonce);
    let mut blocks_cipher = Vec::new();

    for (key_plain, value_plain) in &blocks_plain_original[..blocks_plain_original.len() - 1] {
        let value_cipher = encryptor
            .encrypt_next(Payload {
                msg: value_plain,
                aad: key_plain,
            })
            .unwrap(); // NOTE: this panics if we exhaust the counter*. The counter's max value is
                       //       2^31 so it's unlikely to happen in practice, but we should still
                       //       probably error gracefully instead of crashing even in this unlikely
                       //       case.
                       //
                       //       *) also when the plaintext block or the associated data is too long,
                       //          but we can easily control that.
        blocks_cipher.push((key_plain.clone(), value_cipher));
    }

    // Last block
    let (key_plain, value_plain) = &blocks_plain_original[blocks_plain_original.len() - 1];
    let value_cipher = encryptor
        .encrypt_last(Payload {
            msg: value_plain,
            aad: key_plain,
        })
        .unwrap();
    blocks_cipher.push((key_plain.clone(), value_cipher));

    // Decrypt

    let mut decryptor = Decryptor::new(key.as_array(), nonce);
    let mut blocks_plain_decrypted = Vec::new();

    for (key_plain, value_cipher) in &blocks_cipher[..blocks_plain_original.len() - 1] {
        let value_plain = decryptor
            .decrypt_next(Payload {
                msg: value_cipher,
                aad: key_plain,
            })
            .unwrap();
        blocks_plain_decrypted.push((key_plain.clone(), value_plain));
    }

    // Last block
    let (key_plain, value_cipher) = &blocks_cipher[blocks_cipher.len() - 1];
    let value_plain = decryptor
        .decrypt_last(Payload {
            msg: value_cipher,
            aad: key_plain,
        })
        .unwrap();
    blocks_plain_decrypted.push((key_plain.clone(), value_plain));

    assert_eq!(blocks_plain_original, blocks_plain_decrypted);
}

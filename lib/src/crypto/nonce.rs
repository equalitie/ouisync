use std::mem;

const NONCE_SIZE: usize = 12;
const NONCE_COUNTER_SIZE: usize = mem::size_of::<u32>();
const NONCE_PREFIX_SIZE: usize = NONCE_SIZE - NONCE_COUNTER_SIZE;

pub type Nonce = chacha20poly1305::Nonce;
pub type NoncePrefix = [u8; NONCE_PREFIX_SIZE];

/// Ordered sequence of nonces. Useful when encrypting multiple messages whose order need
/// to be maintained.
#[derive(Clone)]
pub struct NonceSequence {
    prefix: NoncePrefix,
}

impl NonceSequence {
    pub fn new(prefix: NoncePrefix) -> Self {
        Self { prefix }
    }

    /// Gets the shared prefix of each nonce of this sequence.
    pub fn prefix(&self) -> &NoncePrefix {
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

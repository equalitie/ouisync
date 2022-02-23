use crate::crypto::{cipher::SecretKey, Password};
use rand::{CryptoRng, Rng};

#[derive(Clone)]
pub enum MasterSecret {
    Password(Password),
    SecretKey(SecretKey),
}

impl MasterSecret {
    /// Generates random master secret containing a secret key.
    pub fn random() -> Self {
        Self::SecretKey(SecretKey::random())
    }

    /// Generates random master secret containing a secret key using the provided RNG.
    pub fn generate<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        Self::SecretKey(SecretKey::generate(rng))
    }
}

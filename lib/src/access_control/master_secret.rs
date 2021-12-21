use crate::crypto::{cipher::SecretKey, Password};

pub enum MasterSecret {
    Password(Password),
    SecretKey(SecretKey),
}

impl MasterSecret {
    /// Generates random master secret containing a secret key.
    pub fn random() -> Self {
        Self::SecretKey(SecretKey::random())
    }
}

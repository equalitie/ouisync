use crate::crypto::{cipher::SecretKey, Password, PasswordSalt};
use ouisync_macros::api;
#[cfg(test)]
use rand::{CryptoRng, Rng};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[api]
pub enum LocalSecret {
    Password(Password),
    SecretKey(SecretKey),
}

#[cfg(test)]
impl LocalSecret {
    /// Generates random master secret containing a secret key.
    pub fn random() -> Self {
        Self::SecretKey(SecretKey::random())
    }

    /// Generates random master secret containing a secret key using the provided RNG.
    pub fn generate<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        Self::SecretKey(SecretKey::generate(rng))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[api]
pub enum SetLocalSecret {
    Password(Password),
    KeyAndSalt(KeyAndSalt),
}

#[cfg(test)]
impl SetLocalSecret {
    /// Generates random secret key and salt.
    pub fn random() -> Self {
        Self::KeyAndSalt(KeyAndSalt::random())
    }
}

#[cfg(test)]
impl From<SetLocalSecret> for LocalSecret {
    fn from(local: SetLocalSecret) -> Self {
        match local {
            SetLocalSecret::Password(pwd) => Self::Password(pwd),
            SetLocalSecret::KeyAndSalt(local) => Self::SecretKey(local.key),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize, Clone)]
#[api]
pub struct KeyAndSalt {
    pub key: SecretKey,
    pub salt: PasswordSalt,
}

#[cfg(test)]
impl KeyAndSalt {
    /// Generates random secret key and salt.
    pub fn random() -> Self {
        Self {
            key: SecretKey::random(),
            salt: SecretKey::random_salt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_deserialize_bincode() {
        let orig = LocalSecret::Password("mellon".to_string().into());
        let expected_serialized_hex = "0000000006000000000000006d656c6c6f6e";

        let serialized = bincode::serialize(&orig).unwrap();
        assert_eq!(hex::encode(&serialized), expected_serialized_hex);

        let deserialized: LocalSecret = bincode::deserialize(&serialized).unwrap();
        assert_eq!(&deserialized, &orig);
    }

    #[test]
    fn serialize_deserialize_msgpack() {
        let orig = LocalSecret::Password("mellon".to_string().into());
        let expected_serialized_hex = "81a850617373776f7264a66d656c6c6f6e";

        let serialized = rmp_serde::to_vec(&orig).unwrap();
        assert_eq!(hex::encode(&serialized), expected_serialized_hex);

        let deserialized: LocalSecret = rmp_serde::from_slice(&serialized).unwrap();
        assert_eq!(&deserialized, &orig);
    }
}

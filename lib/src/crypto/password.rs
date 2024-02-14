use argon2::password_hash;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, sync::Arc};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// A simple wrapper over String to avoid certain kinds of attack. For more elaboration please see
/// the documentation for the SecretKey structure.
#[derive(Clone)]
pub struct Password(Arc<Zeroizing<String>>);

impl From<String> for Password {
    fn from(pwd: String) -> Self {
        Self(Arc::new(Zeroizing::new(pwd)))
    }
}

impl AsRef<str> for Password {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

/// Note this impl uses constant-time operations (using [subtle](https://crates.io/crates/subtle))
/// and so provides protection against software side-channel attacks.
impl PartialEq for Password {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
    }
}

impl Eq for Password {}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "****")
    }
}

impl Serialize for Password {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_ref().serialize(s)
    }
}

impl<'de> Deserialize<'de> for Password {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from(String::deserialize(d)?))
    }
}

pub(crate) const PASSWORD_SALT_LEN: usize = password_hash::Salt::RECOMMENDED_LENGTH;
pub type PasswordSalt = [u8; PASSWORD_SALT_LEN];

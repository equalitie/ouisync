use argon2::password_hash;
use ouisync_macros::api;
use rand::{rngs::OsRng, Rng};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use std::{array::TryFromSliceError, fmt, sync::Arc};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// A simple wrapper over String to avoid certain kinds of attack. For more elaboration please see
/// the documentation for the SecretKey structure.
#[derive(Clone)]
#[api(repr(String), secret)]
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

#[derive(Copy, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
#[api(repr(Bytes))]
pub struct PasswordSalt([u8; Self::SIZE]);

// NOTE: Not using the `define_byte_array_wrapper` macro here because this type needs to be visible
// to the API parser which is currently not smart emough to see inside macros.
// TODO: consider changing `define_byte_array_wrapper` to a proc-macro

impl PasswordSalt {
    pub const SIZE: usize = password_hash::Salt::RECOMMENDED_LENGTH;

    pub fn as_array(&self) -> &[u8; Self::SIZE] {
        &self.0
    }

    pub fn random() -> Self {
        OsRng.gen()
    }
}

impl From<[u8; Self::SIZE]> for PasswordSalt {
    fn from(array: [u8; Self::SIZE]) -> Self {
        Self(array)
    }
}

impl TryFrom<&'_ [u8]> for PasswordSalt {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(slice.try_into()?))
    }
}

impl AsRef<[u8]> for PasswordSalt {
    fn as_ref(&self) -> &[u8] {
        &self.0[..]
    }
}

impl fmt::Debug for PasswordSalt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:<8x}", self)
    }
}

impl fmt::LowerHex for PasswordSalt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        hex_fmt::HexFmt(&self.0).fmt(f)
    }
}

impl Serialize for PasswordSalt {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_bytes::Bytes::new(self.as_ref()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for PasswordSalt {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let bytes: &serde_bytes::Bytes = Deserialize::deserialize(d)?;

        if bytes.len() != Self::SIZE {
            return Err(D::Error::invalid_length(
                bytes.len(),
                &format!("{}", Self::SIZE).as_str(),
            ));
        }

        let mut salt = [0; Self::SIZE];
        salt.as_mut().copy_from_slice(bytes);

        Ok(PasswordSalt(salt))
    }
}

derive_rand_for_wrapper!(PasswordSalt);

mod access_mode;
mod local_secret;
mod share_token;

pub use self::{access_mode::AccessMode, local_secret::LocalSecret, share_token::ShareToken};

use crate::{
    crypto::{cipher, sign},
    error::Error,
    repository::RepositoryId,
    Result,
};
use rand::{rngs::OsRng, CryptoRng, Rng};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::Utf8Error, string::FromUtf8Error, sync::Arc};
use thiserror::Error;

/// Secrets for access to a repository.
#[derive(Clone, Serialize, Deserialize)]
pub enum AccessSecrets {
    Blind {
        id: RepositoryId,
    },
    Read {
        id: RepositoryId,
        read_key: cipher::SecretKey,
    },
    Write(WriteSecrets),
}

impl AccessSecrets {
    /// Generates random access secrets with write access using the provided RNG.
    pub fn generate_write<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        Self::Write(WriteSecrets::generate(rng))
    }

    /// Generates random access secrets with write access using OsRng.
    pub fn random_write() -> Self {
        Self::Write(WriteSecrets::random())
    }

    /// Change the access mode of this secrets to the given mode. If the given mode is higher than
    /// self, returns self unchanged.
    pub fn with_mode(&self, mode: AccessMode) -> Self {
        match (self, mode) {
            (Self::Blind { .. }, AccessMode::Blind | AccessMode::Read | AccessMode::Write)
            | (Self::Read { .. }, AccessMode::Read | AccessMode::Write)
            | (Self::Write { .. }, AccessMode::Write) => self.clone(),
            (Self::Read { id, .. } | Self::Write(WriteSecrets { id, .. }), AccessMode::Blind) => {
                Self::Blind { id: *id }
            }
            (Self::Write(WriteSecrets { id, read_key, .. }), AccessMode::Read) => Self::Read {
                id: *id,
                read_key: read_key.clone(),
            },
        }
    }

    pub fn access_mode(&self) -> AccessMode {
        match self {
            Self::Blind { .. } => AccessMode::Blind,
            Self::Read { .. } => AccessMode::Read,
            Self::Write(_) => AccessMode::Write,
        }
    }

    pub fn id(&self) -> &RepositoryId {
        match self {
            Self::Blind { id } | Self::Read { id, .. } | Self::Write(WriteSecrets { id, .. }) => id,
        }
    }

    pub(crate) fn can_write(&self) -> bool {
        matches!(self, Self::Write(_))
    }

    pub(crate) fn can_read(&self) -> bool {
        matches!(self, Self::Read { .. } | Self::Write(_))
    }

    pub(crate) fn read_key(&self) -> Option<&cipher::SecretKey> {
        match self {
            Self::Blind { .. } => None,
            Self::Read { read_key, .. } => Some(read_key),
            Self::Write(secrets) => Some(&secrets.read_key),
        }
    }

    pub(crate) fn write_secrets(&self) -> Option<&WriteSecrets> {
        match self {
            Self::Blind { .. } => None,
            Self::Read { .. } => None,
            Self::Write(secrets) => Some(secrets),
        }
    }

    pub(crate) fn keys(&self) -> Option<AccessKeys> {
        match self {
            Self::Blind { .. } => None,
            Self::Read { read_key, .. } => Some(AccessKeys {
                read: read_key.clone(),
                write: None,
            }),
            Self::Write(secrets) => Some(AccessKeys {
                read: secrets.read_key.clone(),
                write: Some(secrets.write_keys.clone()),
            }),
        }
    }
}

impl fmt::Debug for AccessSecrets {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Blind { .. } => f.debug_struct("Blind").finish_non_exhaustive(),
            Self::Read { .. } => f.debug_struct("Read").finish_non_exhaustive(),
            Self::Write { .. } => f.debug_struct("Write").finish_non_exhaustive(),
        }
    }
}

impl PartialEq for AccessSecrets {
    fn eq(&self, other: &Self) -> bool {
        self.access_mode() == other.access_mode() && self.id() == other.id()
    }
}

impl Eq for AccessSecrets {}

/// Secrets for write access.
#[derive(Clone)]
pub struct WriteSecrets {
    pub(crate) id: RepositoryId,
    pub(crate) read_key: cipher::SecretKey,
    pub(crate) write_keys: Arc<sign::Keypair>,
}

impl WriteSecrets {
    /// Generates random write secrets using the provided RNG.
    pub fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        Self::from(sign::Keypair::generate(rng))
    }

    /// Generates random write secrets using OsRng.
    pub fn random() -> Self {
        Self::generate(&mut OsRng)
    }
}

impl PartialEq for WriteSecrets {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for WriteSecrets {}

impl From<sign::Keypair> for WriteSecrets {
    fn from(keys: sign::Keypair) -> Self {
        let id = keys.public_key().into();
        let read_key = derive_read_key_from_write_keys(&keys);

        Self {
            id,
            read_key,
            write_keys: Arc::new(keys),
        }
    }
}

impl Serialize for WriteSecrets {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize only the write keys because all the other fields can be derived from it
        self.write_keys.serialize(s)
    }
}

impl<'de> Deserialize<'de> for WriteSecrets {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::from(sign::Keypair::deserialize(d)?))
    }
}

/// Secret keys for read and optionaly write access.
#[derive(Clone)]
pub(crate) struct AccessKeys {
    read: cipher::SecretKey,
    write: Option<Arc<sign::Keypair>>,
}

impl AccessKeys {
    pub fn read(&self) -> &cipher::SecretKey {
        &self.read
    }

    pub fn write(&self) -> Option<&sign::Keypair> {
        self.write.as_deref()
    }

    pub fn read_only(self) -> Self {
        Self {
            read: self.read,
            write: None,
        }
    }
}

impl From<WriteSecrets> for AccessKeys {
    fn from(secrets: WriteSecrets) -> Self {
        Self {
            read: secrets.read_key,
            write: Some(secrets.write_keys),
        }
    }
}

fn derive_read_key_from_write_keys(write_keys: &sign::Keypair) -> cipher::SecretKey {
    cipher::SecretKey::derive_from_key(&write_keys.to_bytes(), b"ouisync repository read key")
}

#[derive(Debug, Error)]
#[error("decode error")]
pub struct DecodeError;

impl From<base64::DecodeError> for DecodeError {
    fn from(_: base64::DecodeError) -> Self {
        Self
    }
}

impl From<bincode::Error> for DecodeError {
    fn from(_: bincode::Error) -> Self {
        Self
    }
}

impl From<FromUtf8Error> for DecodeError {
    fn from(_: FromUtf8Error) -> Self {
        Self
    }
}

impl From<Utf8Error> for DecodeError {
    fn from(_: Utf8Error) -> Self {
        Self
    }
}

impl From<sign::SignatureError> for DecodeError {
    fn from(_: sign::SignatureError) -> Self {
        Self
    }
}

impl From<cipher::SecretKeyLengthError> for DecodeError {
    fn from(_: cipher::SecretKeyLengthError) -> Self {
        Self
    }
}

impl From<DecodeError> for Error {
    fn from(_: DecodeError) -> Self {
        Self::MalformedData
    }
}

pub enum Access {
    // User has no read nor write access, can only sync.
    Blind {
        id: RepositoryId,
    },
    // User doesn't need a secret to read the repository, there's no write access.
    ReadUnlocked {
        id: RepositoryId,
        read_key: cipher::SecretKey,
    },
    // Providing a secret will grant the user read access, there's no write access.
    ReadLocked {
        id: RepositoryId,
        local_secret: LocalSecret,
        read_key: cipher::SecretKey,
    },
    // User doesn't need a secret to read nor write.
    WriteUnlocked {
        secrets: WriteSecrets,
    },
    // Providing a secret user will grant read and write access. The secret may be different for
    // reading or writing.
    WriteLocked {
        local_read_secret: LocalSecret,
        local_write_secret: LocalSecret,
        secrets: WriteSecrets,
    },
    // User doesn't need a secret to read, but a secret will grant access to write.
    WriteLockedReadUnlocked {
        local_write_secret: LocalSecret,
        secrets: WriteSecrets,
    },
}

impl Access {
    pub fn new(
        local_read_secret: Option<LocalSecret>,
        local_write_secret: Option<LocalSecret>,
        secrets: AccessSecrets,
    ) -> Self {
        match (local_read_secret, local_write_secret, secrets) {
            (_, _, AccessSecrets::Blind { id }) => Access::Blind { id },
            (None, _, AccessSecrets::Read { id, read_key }) => {
                Access::ReadUnlocked { id, read_key }
            }
            (Some(local_read_secret), _, AccessSecrets::Read { id, read_key }) => {
                Access::ReadLocked {
                    id,
                    local_secret: local_read_secret,
                    read_key,
                }
            }
            (None, None, AccessSecrets::Write(secrets)) => Access::WriteUnlocked { secrets },
            (Some(local_read_secret), None, AccessSecrets::Write(secrets)) => Access::ReadLocked {
                id: secrets.id,
                local_secret: local_read_secret,
                read_key: secrets.read_key,
            },
            (None, Some(local_write_secret), AccessSecrets::Write(secrets)) => {
                Access::WriteLockedReadUnlocked {
                    local_write_secret,
                    secrets,
                }
            }
            (Some(local_read_secret), Some(local_write_secret), AccessSecrets::Write(secrets)) => {
                Access::WriteLocked {
                    local_read_secret,
                    local_write_secret,
                    secrets,
                }
            }
        }
    }

    pub fn id(&self) -> &RepositoryId {
        match self {
            Self::Blind { id } => id,
            Self::ReadUnlocked { id, .. } => id,
            Self::ReadLocked { id, .. } => id,
            Self::WriteUnlocked { secrets } => &secrets.id,
            Self::WriteLocked { secrets, .. } => &secrets.id,
            Self::WriteLockedReadUnlocked { secrets, .. } => &secrets.id,
        }
    }

    pub fn secrets(self) -> AccessSecrets {
        match self {
            Self::Blind { id } => AccessSecrets::Blind { id },
            Self::ReadUnlocked { id, read_key } => AccessSecrets::Read { id, read_key },
            Self::ReadLocked { id, read_key, .. } => AccessSecrets::Read { id, read_key },
            Self::WriteUnlocked { secrets } => AccessSecrets::Write(secrets),
            Self::WriteLocked { secrets, .. } => AccessSecrets::Write(secrets),
            Self::WriteLockedReadUnlocked { secrets, .. } => AccessSecrets::Write(secrets),
        }
    }

    pub fn local_write_secret(&self) -> Option<&LocalSecret> {
        match self {
            Self::WriteLocked {
                local_write_secret, ..
            } => Some(local_write_secret),
            Self::WriteLockedReadUnlocked {
                local_write_secret, ..
            } => Some(local_write_secret),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn highest_local_secret(&self) -> Option<&LocalSecret> {
        match self {
            Self::Blind { .. } => None,
            Self::ReadUnlocked { .. } => None,
            Self::ReadLocked { local_secret, .. } => Some(local_secret),
            Self::WriteUnlocked { .. } => None,
            Self::WriteLocked {
                local_write_secret, ..
            } => Some(local_write_secret),
            Self::WriteLockedReadUnlocked {
                local_write_secret, ..
            } => Some(local_write_secret),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessChange {
    Enable(Option<LocalSecret>),
    Disable,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note we don't actually use JSON anywhere in the protocol but this test uses it because it
    // being human readable makes it easy to verify the values are serialized the way we want them.
    #[test]
    fn access_change_serialize_deserialize_json() {
        for (orig, expected_serialized) in [
            (
                AccessChange::Enable(Some(LocalSecret::Password("mellon".to_string().into()))),
                "{\"enable\":\"mellon\"}",
            ),
            (AccessChange::Enable(None), "{\"enable\":null}"),
            (AccessChange::Disable, "\"disable\""),
        ] {
            let serialized = serde_json::to_string(&orig).unwrap();
            assert_eq!(serialized, expected_serialized);

            let deserialized: AccessChange = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, orig);
        }
    }

    #[test]
    fn access_change_serialize_deserialize_msgpack() {
        for (orig, expected_serialized_hex) in [
            (
                AccessChange::Enable(Some(LocalSecret::Password("mellon".to_string().into()))),
                "81a6656e61626c65a66d656c6c6f6e",
            ),
            (AccessChange::Enable(None), "81a6656e61626c65c0"),
            (AccessChange::Disable, "a764697361626c65"),
        ] {
            let serialized = rmp_serde::to_vec(&orig).unwrap();
            assert_eq!(hex::encode(&serialized), expected_serialized_hex);

            let deserialized: AccessChange = rmp_serde::from_slice(&serialized).unwrap();
            assert_eq!(deserialized, orig);
        }
    }
}

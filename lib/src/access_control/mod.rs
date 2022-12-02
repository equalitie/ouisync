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
use std::{fmt, str::Utf8Error, string::FromUtf8Error, sync::Arc};
use thiserror::Error;

/// Secrets for access to a repository.
#[derive(Clone)]
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

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Blind { id } => {
                out.push(AccessMode::Blind as u8);
                out.extend_from_slice(id.as_ref());
            }
            Self::Read { id, read_key } => {
                out.push(AccessMode::Read as u8);
                out.extend_from_slice(id.as_ref());
                out.extend_from_slice(read_key.as_ref());
            }
            Self::Write(secrets) => {
                out.push(AccessMode::Write as u8);
                out.extend_from_slice(secrets.write_keys.secret.as_ref());
            }
        }
    }

    // Returns the decoded secrets and the remaining input.
    pub(crate) fn decode(input: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        let (mode, input) = input.split_first().ok_or(DecodeError)?;
        let mode = AccessMode::try_from(*mode)?;

        match mode {
            AccessMode::Blind => {
                let (id, input) = try_split_at(input, RepositoryId::SIZE).ok_or(DecodeError)?;
                let id = RepositoryId::try_from(id)?;

                Ok((Self::Blind { id }, input))
            }
            AccessMode::Read => {
                let (id, input) = try_split_at(input, RepositoryId::SIZE).ok_or(DecodeError)?;
                let id = RepositoryId::try_from(id)?;

                let (read_key, input) =
                    try_split_at(input, cipher::SecretKey::SIZE).ok_or(DecodeError)?;
                let read_key = cipher::SecretKey::try_from(read_key)?;

                Ok((Self::Read { id, read_key }, input))
            }
            AccessMode::Write => {
                let (write_key, input) =
                    try_split_at(input, sign::SecretKey::SIZE).ok_or(DecodeError)?;
                let write_key = sign::SecretKey::try_from(write_key)?;
                let write_keys = sign::Keypair::from(write_key);

                Ok((Self::Write(write_keys.into()), input))
            }
        }
    }

    pub(crate) fn can_write(&self) -> bool {
        matches!(self, Self::Write(_))
    }

    pub(crate) fn can_read(&self) -> bool {
        matches!(self, Self::Read { .. } | Self::Write(_))
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

#[cfg(test)]
impl PartialEq for AccessSecrets {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AccessSecrets::Blind { id: id1 }, AccessSecrets::Blind { id: id2 }) => id1.eq(id2),
            (
                AccessSecrets::Read {
                    id: id1,
                    read_key: k1,
                },
                AccessSecrets::Read {
                    id: id2,
                    read_key: k2,
                },
            ) => id1.eq(id2) && k1 == k2,
            (AccessSecrets::Write(ss), AccessSecrets::Write(os)) => ss.eq(os),
            _ => false,
        }
    }
}

#[cfg(test)]
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

#[cfg(test)]
impl PartialEq for WriteSecrets {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.read_key == other.read_key
            && self.write_keys.public == other.write_keys.public
    }
}

#[cfg(test)]
impl Eq for WriteSecrets {}

impl From<sign::Keypair> for WriteSecrets {
    fn from(keys: sign::Keypair) -> Self {
        let id = keys.public.into();
        let read_key = derive_read_key_from_write_key(&keys.secret);

        Self {
            id,
            read_key,
            write_keys: Arc::new(keys),
        }
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

fn derive_read_key_from_write_key(write_key: &sign::SecretKey) -> cipher::SecretKey {
    cipher::SecretKey::derive_from_key(write_key.as_ref(), b"ouisync repository read key")
}

// Similar to `split_at` but returns `None` instead of panic when `index` is out of range.
fn try_split_at(slice: &[u8], index: usize) -> Option<(&[u8], &[u8])> {
    if index <= slice.len() {
        Some(slice.split_at(index))
    } else {
        None
    }
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
        local_key: cipher::SecretKey,
        read_key: cipher::SecretKey,
    },
    // User doesn't need a secret to read nor write.
    WriteUnlocked {
        secrets: WriteSecrets,
    },
    // Providing a secret user will grant read and write access. The secret may be different for
    // reading or writing.
    WriteLocked {
        local_read_key: cipher::SecretKey,
        local_write_key: cipher::SecretKey,
        secrets: WriteSecrets,
    },
    // User doesn't need a secret to read, but a secret will grant access to write.
    WriteLockedReadUnlocked {
        local_write_key: cipher::SecretKey,
        secrets: WriteSecrets,
    },
}

impl Access {
    pub fn new(
        local_read_key: Option<cipher::SecretKey>,
        local_write_key: Option<cipher::SecretKey>,
        secrets: AccessSecrets,
    ) -> Self {
        match (local_read_key, local_write_key, secrets) {
            (_, _, AccessSecrets::Blind { id }) => Access::Blind { id },
            (None, _, AccessSecrets::Read { id, read_key }) => {
                Access::ReadUnlocked { id, read_key }
            }
            (Some(local_read_key), _, AccessSecrets::Read { id, read_key }) => Access::ReadLocked {
                id,
                local_key: local_read_key,
                read_key,
            },
            (None, None, AccessSecrets::Write(secrets)) => Access::WriteUnlocked { secrets },
            (Some(local_read_key), None, AccessSecrets::Write(secrets)) => Access::ReadLocked {
                id: secrets.id,
                local_key: local_read_key,
                read_key: secrets.read_key,
            },
            (None, Some(local_write_key), AccessSecrets::Write(secrets)) => {
                Access::WriteLockedReadUnlocked {
                    local_write_key,
                    secrets,
                }
            }
            (Some(local_read_key), Some(local_write_key), AccessSecrets::Write(secrets)) => {
                Access::WriteLocked {
                    local_read_key,
                    local_write_key,
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

    pub fn local_write_key(&self) -> Option<&cipher::SecretKey> {
        match self {
            Self::WriteLocked {
                local_write_key, ..
            } => Some(local_write_key),
            Self::WriteLockedReadUnlocked {
                local_write_key, ..
            } => Some(local_write_key),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn highest_local_key(&self) -> Option<&cipher::SecretKey> {
        match self {
            Self::Blind { .. } => None,
            Self::ReadUnlocked { .. } => None,
            Self::ReadLocked { local_key, .. } => Some(local_key),
            Self::WriteUnlocked { .. } => None,
            Self::WriteLocked {
                local_write_key, ..
            } => Some(local_write_key),
            Self::WriteLockedReadUnlocked {
                local_write_key, ..
            } => Some(local_write_key),
        }
    }
}

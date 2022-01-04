mod access_mode;
mod master_secret;
mod share_token;

pub use self::{access_mode::AccessMode, master_secret::MasterSecret, share_token::ShareToken};

use crate::{
    crypto::{cipher, sign},
    error::Error,
    repository::RepositoryId,
};
use rand::{rngs::OsRng, CryptoRng, Rng};
use std::{fmt, string::FromUtf8Error, sync::Arc};
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

    pub(crate) fn id(&self) -> &RepositoryId {
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

    pub(crate) fn decode(mut input: &[u8]) -> Result<Self, DecodeError> {
        let mode = *input.get(0).ok_or(DecodeError)?;
        let mode = AccessMode::try_from(mode)?;
        input = &input[1..];

        match mode {
            AccessMode::Blind => {
                let id = RepositoryId::try_from(input)?;
                Ok(Self::Blind { id })
            }
            AccessMode::Read => {
                let id = RepositoryId::try_from(&input[..RepositoryId::SIZE])?;
                let read_key = cipher::SecretKey::try_from(&input[RepositoryId::SIZE..])?;
                Ok(Self::Read { id, read_key })
            }
            AccessMode::Write => {
                let write_key = sign::SecretKey::try_from(input)?;
                let write_keys = sign::Keypair::from(write_key);
                Ok(Self::Write(write_keys.into()))
            }
        }
    }

    pub(crate) fn can_write(&self) -> bool {
        matches!(self, Self::Write(_))
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

/// Secrets for write access.
#[derive(Clone)]
pub struct WriteSecrets {
    pub(crate) id: RepositoryId,
    pub(crate) read_key: cipher::SecretKey,
    pub(crate) write_keys: Arc<sign::Keypair>,
}

impl WriteSecrets {
    /// Generates random write secrets using the provided RNG.
    pub(crate) fn generate<R: Rng + CryptoRng>(rng: &mut R) -> Self {
        Self::from(sign::Keypair::generate(rng))
    }

    /// Generates random write secrets using OsRng.
    pub(crate) fn random() -> Self {
        Self::generate(&mut OsRng)
    }
}

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

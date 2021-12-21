mod master_secret;
mod share_token;

pub use self::{master_secret::MasterSecret, share_token::ShareToken};

use crate::{
    crypto::{cipher, sign, Cryptor},
    error::Error,
    repository::RepositoryId,
};
use rand::{rngs::OsRng, CryptoRng, Rng};
use std::{fmt, str::FromStr, string::FromUtf8Error, sync::Arc};
use thiserror::Error;

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
    pub fn generate_write<R: Rng + CryptoRng + ?Sized>(rng: &mut R) -> Self {
        let write_key: sign::SecretKey = rng.gen();
        let write_secrets = WriteSecrets::from(write_key);

        Self::Write(write_secrets)
    }

    /// Generates random access secrets with write access using OsRng.
    pub fn random_write() -> Self {
        Self::generate_write(&mut OsRng)
    }

    /// Change the access mode of this secrets to the given mode. If the given mode is higher than
    /// self, returns self unchanged.
    pub fn with_mode(self: Arc<Self>, mode: AccessMode) -> Arc<Self> {
        match (self.as_ref(), mode) {
            (Self::Blind { .. }, AccessMode::Blind | AccessMode::Read | AccessMode::Write)
            | (Self::Read { .. }, AccessMode::Read | AccessMode::Write)
            | (Self::Write { .. }, AccessMode::Write) => self,
            (Self::Read { id, .. } | Self::Write(WriteSecrets { id, .. }), AccessMode::Blind) => {
                Arc::new(Self::Blind { id: *id })
            }
            (Self::Write(WriteSecrets { id, read_key, .. }), AccessMode::Read) => {
                Arc::new(Self::Read {
                    id: *id,
                    read_key: read_key.clone(),
                })
            }
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
                out.extend_from_slice(secrets.write_key.as_ref());
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
                Ok(Self::Write(write_key.into()))
            }
        }
    }

    pub(crate) fn can_write(&self) -> bool {
        matches!(self, Self::Write(_))
    }

    // TODO: temporary method, remove when the integration of AccessSecrets is done.
    pub(crate) fn cryptor(&self) -> Cryptor {
        match self {
            Self::Blind { .. } => Cryptor::Null,
            Self::Read { read_key, .. } | Self::Write(WriteSecrets { read_key, .. }) => {
                Cryptor::ChaCha20Poly1305(read_key.clone())
            }
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

pub struct WriteSecrets {
    pub(crate) id: RepositoryId,
    pub(crate) read_key: cipher::SecretKey,
    pub(crate) write_key: sign::SecretKey,
}

impl From<sign::SecretKey> for WriteSecrets {
    fn from(write_key: sign::SecretKey) -> Self {
        let id = sign::PublicKey::from(&write_key);
        let id = id.into();

        let read_key = derive_read_key_from_write_key(&write_key);

        Self {
            id,
            read_key,
            write_key,
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum AccessMode {
    Blind = 0,
    Read = 1,
    Write = 2,
}

impl TryFrom<u8> for AccessMode {
    type Error = DecodeError;

    fn try_from(byte: u8) -> Result<Self, Self::Error> {
        match byte {
            b if b == Self::Blind as u8 => Ok(Self::Blind),
            b if b == Self::Read as u8 => Ok(Self::Read),
            b if b == Self::Write as u8 => Ok(Self::Write),
            _ => Err(DecodeError),
        }
    }
}

impl FromStr for AccessMode {
    type Err = AccessModeParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.chars().next() {
            Some('b' | 'B') => Ok(AccessMode::Blind),
            Some('r' | 'R') => Ok(AccessMode::Read),
            Some('w' | 'W') => Ok(AccessMode::Write),
            _ => Err(AccessModeParseError),
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

#[derive(Debug, Error)]
#[error("failed to parse access mode")]
pub struct AccessModeParseError;

mod share_token;

pub use self::share_token::ShareToken;

use crate::{
    crypto::{sign, SecretKey},
    repository::RepositoryId,
};
use std::fmt;

pub(crate) enum AccessSecrets {
    Blind {
        id: RepositoryId,
    },
    Read {
        id: RepositoryId,
        read_key: SecretKey,
    },
    Write(WriteSecrets),
}

impl AccessSecrets {
    pub fn id(&self) -> &RepositoryId {
        match self {
            Self::Blind { id } | Self::Read { id, .. } | Self::Write(WriteSecrets { id, .. }) => id,
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

pub(crate) struct WriteSecrets {
    pub id: RepositoryId,
    pub read_key: SecretKey,
    pub write_key: sign::SecretKey,
}

impl WriteSecrets {
    pub fn new(write_key: sign::SecretKey) -> Self {
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

fn derive_read_key_from_write_key(write_key: &sign::SecretKey) -> SecretKey {
    SecretKey::derive_from_key(write_key.as_ref(), b"ouisync repository read key")
}

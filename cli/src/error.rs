/*
use crate::repository::RepositoryNameInvalid;
use ouisync_bridge::{config::ConfigError, protocol::remote::ServerError, repository::OpenError};
use ouisync_lib::StoreError;
use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("repository error")]
    Repository(#[from] ouisync_lib::Error),
    #[error("repository already exists")]
    RepositoryExists,
    #[error("repository not found")]
    RepositoryNotFound,
    #[error("repository name is invalid")]
    RepositoryNameInvalid,
    #[error("config error")]
    Config(#[from] ConfigError),
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("TLS certificates not found")]
    TlsCertificatesNotFound,
    #[error("TLS keys not found")]
    TlsKeysNotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("server error")]
    Server(#[from] ServerError),
}

impl From<RepositoryNameInvalid> for Error {
    fn from(_: RepositoryNameInvalid) -> Self {
        Self::RepositoryNameInvalid
    }
}

impl From<OpenError> for Error {
    fn from(error: OpenError) -> Self {
        match error {
            OpenError::Config(error) => Self::Config(error),
            OpenError::Repository(error) => Self::Repository(error),
        }
    }
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Self::Repository(error.into())
    }
}
*/

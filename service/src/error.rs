use crate::transport::{AcceptError, BindError};
use ouisync_bridge::{config::ConfigError, repository::OpenError};
use ouisync_vfs::MountError;
use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("config error")]
    Config(#[from] ConfigError),
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("mount error")]
    Mount(#[from] MountError),
    #[error("operation not supported")]
    OperationNotSupported,
    #[error("repository error")]
    Repository(#[from] ouisync::Error),
    #[error("repository already exists")]
    RepositoryExists,
    #[error("repository not found")]
    RepositoryNotFound,
    #[error("repository sync is disabled")]
    RepositorySyncDisabled,
    #[error("store error")]
    Store(#[from] ouisync::StoreError),
    #[error("TLS certificates not found")]
    TlsCertificatesNotFound,
    #[error("TLS keys not found")]
    TlsKeysNotFound,
    #[error("server bind error")]
    Bind(#[from] BindError),
    #[error("server accept error")]
    Accept(#[from] AcceptError),
}

impl From<OpenError> for Error {
    fn from(src: OpenError) -> Self {
        match src {
            OpenError::Repository(error) => Self::Repository(error),
            OpenError::Config(error) => Self::Config(error),
        }
    }
}

use crate::{config_store::ConfigError, repository::FindError, transport::ClientError};
use ouisync_vfs::MountError;
use std::{ffi::IntoStringError, io, str::Utf8Error};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("entry already exists")]
    AlreadyExists,
    #[error("config error")]
    Config(#[from] ConfigError),
    #[error("failed to create mounter")]
    CreateMounter(#[from] MountError),
    #[error("failed to initialize logger")]
    InitializeLogger(#[source] io::Error),
    #[error("failed to initialize runtime")]
    InitializeRuntime(#[source] io::Error),
    #[error("argument is invalid")]
    InvalidArgument,
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("entry not found")]
    NotFound,
    #[error("name is ambiguous")]
    Ambiguous,
    #[error("operation not supported")]
    OperationNotSupported,
    #[error("permission denied")]
    PermissionDenied,
    #[error("repository error")]
    Repository(#[from] ouisync::Error),
    #[error("store error")]
    Store(#[from] ouisync::StoreError),
    #[error("store dir not specified")]
    StoreDirUnspecified,
    #[error("TLS certificates not found")]
    TlsCertificatesNotFound,
    #[error("TLS certificates failed to load")]
    TlsCertificatesInvalid(#[source] io::Error),
    #[error("TLS keys not found")]
    TlsKeysNotFound,
    #[error("failed to create TLS config")]
    TlsConfig(#[source] tokio_rustls::rustls::Error),
    #[error("service is already running")]
    ServiceAlreadyRunning,
    #[error("failed to bind server")]
    Bind(#[source] io::Error),
    #[error("failed to accept client connection")]
    Accept(#[source] io::Error),
    #[error("client request failed")]
    Client(#[from] ClientError),
}

impl From<Utf8Error> for Error {
    fn from(_: Utf8Error) -> Self {
        Self::InvalidArgument
    }
}

impl From<IntoStringError> for Error {
    fn from(_: IntoStringError) -> Self {
        Self::InvalidArgument
    }
}

impl From<FindError> for Error {
    fn from(e: FindError) -> Self {
        match e {
            FindError::NotFound => Self::NotFound,
            FindError::Ambiguous => Self::Ambiguous,
        }
    }
}

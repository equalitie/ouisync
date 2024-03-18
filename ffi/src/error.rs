use crate::{
    registry::InvalidHandle,
    repository::{EntryChanged, RegistrationRequired},
    session::SessionError,
};
use num_enum::{IntoPrimitive, TryFromPrimitive};
use ouisync_bridge::{
    config::ConfigError,
    protocol::remote::ServerError,
    repository::{OpenError, RemoteError},
    transport::TransportError,
};
use ouisync_vfs::MountError;
use serde::{Deserialize, Serialize};
use std::io;
use thiserror::Error;

#[derive(Debug, Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(
    Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, IntoPrimitive, TryFromPrimitive,
)]
#[repr(u16)]
#[serde(into = "u16", try_from = "u16")]
pub enum ErrorCode {
    /// No error
    Ok = 0,
    /// Store error
    Store = 1,
    /// Insuficient permission to perform the intended operation
    PermissionDenied = 2,
    /// Malformed data
    MalformedData = 3,
    /// Entry already exists
    EntryExists = 4,
    /// Entry doesn't exist
    EntryNotFound = 5,
    /// Multiple matching entries found
    AmbiguousEntry = 6,
    /// The intended operation requires the directory to be empty but it isn't
    DirectoryNotEmpty = 7,
    /// The indended operation is not supported
    OperationNotSupported = 8,
    /// Failed to read from or write into the config file
    Config = 10,
    /// Argument passed to a function is not valid
    InvalidArgument = 11,
    /// Request or response is malformed
    MalformedMessage = 12,
    /// Storage format version mismatch
    StorageVersionMismatch = 13,
    /// Connection lost
    ConnectionLost = 14,
    /// Invalid handle to a resource (e.g., Repository, File, ...)
    InvalidHandle = 15,
    /// Entry has been changed and no longer matches the expected value
    EntryChanged = 16,

    VfsInvalidMountPoint = 2048,
    VfsDriverInstall = 2048 + 1,
    VfsBackend = 2048 + 2,

    /// Unspecified error
    Other = 65535,
}

pub(crate) trait ToErrorCode {
    fn to_error_code(&self) -> ErrorCode;
}

impl ToErrorCode for SessionError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::InitializeLogger(_) | Self::InitializeRuntime(_) => ErrorCode::Other,
            Self::InvalidUtf8(_) => ErrorCode::InvalidArgument,
        }
    }
}

impl ToErrorCode for RemoteError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::Connect(error) => error.to_error_code(),
            Self::Server(error) => error.to_error_code(),
        }
    }
}

impl ToErrorCode for ouisync_lib::Error {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Db(_) | Self::Store(_) => ErrorCode::Store,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::MalformedData | Self::MalformedDirectory => ErrorCode::MalformedData,
            Self::EntryExists => ErrorCode::EntryExists,
            Self::EntryNotFound => ErrorCode::EntryNotFound,
            Self::AmbiguousEntry => ErrorCode::AmbiguousEntry,
            Self::DirectoryNotEmpty => ErrorCode::DirectoryNotEmpty,
            Self::OperationNotSupported => ErrorCode::OperationNotSupported,
            Self::InvalidArgument | Self::NonUtf8FileName | Self::OffsetOutOfRange => {
                ErrorCode::InvalidArgument
            }
            Self::StorageVersionMismatch => ErrorCode::StorageVersionMismatch,
            Self::EntryIsFile | Self::EntryIsDirectory | Self::Writer(_) | Self::Locked => {
                ErrorCode::Other
            }
        }
    }
}

impl ToErrorCode for ouisync_lib::StoreError {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::Store
    }
}

impl ToErrorCode for TransportError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            TransportError::ConnectionLost => ErrorCode::ConnectionLost,
            TransportError::MalformedMessage => ErrorCode::MalformedMessage,
        }
    }
}

impl ToErrorCode for ServerError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::ShuttingDown => ErrorCode::Other,
            Self::Transport(error) => error.to_error_code(),
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::NotFound => ErrorCode::EntryNotFound,
            Self::Internal(_) => ErrorCode::Other,
        }
    }
}

impl ToErrorCode for ConfigError {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::Config
    }
}

impl ToErrorCode for OpenError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Config(error) => error.to_error_code(),
            Self::Repository(error) => error.to_error_code(),
        }
    }
}

impl ToErrorCode for MountError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::InvalidMountPoint => ErrorCode::VfsInvalidMountPoint,
            Self::Unsupported => ErrorCode::OperationNotSupported,
            Self::DriverInstall => ErrorCode::VfsDriverInstall,
            Self::Backend(_) => ErrorCode::VfsBackend,
        }
    }
}

impl ToErrorCode for InvalidHandle {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::InvalidHandle
    }
}

impl ToErrorCode for RegistrationRequired {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::OperationNotSupported
    }
}

impl ToErrorCode for EntryChanged {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::EntryChanged
    }
}

impl ToErrorCode for io::Error {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::Other
    }
}

impl<T> From<T> for Error
where
    T: std::error::Error + ToErrorCode,
{
    fn from(src: T) -> Self {
        Self {
            code: src.to_error_code(),
            message: src.to_string(),
        }
    }
}

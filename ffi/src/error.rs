use crate::SessionError;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use ouisync_bridge::{
    protocol::remote::ServerError,
    repository::{MirrorError, OpenError},
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

    /// Failed to parse the mount point string
    VfsFailedToParseMountPoint = 2048,

    /// Mounting is not yes supported on this Operating System
    VfsUnsupportedOs = 2048 + 1,

    // These are equivalents of the dokan::file_system::FileSystemMountError errors
    // https://github.com/dokan-dev/dokan-rust/blob/master/dokan/src/file_system.rs
    /// A general error
    VfsGeneral = 2048 + 3,
    /// Bad drive letter
    VfsDriveLetter = 2048 + 4,
    /// Can't install the Dokan driver.
    VfsDriverInstall = 2048 + 5,
    /// The driver responds that something is wrong.
    VfsStart = 2048 + 2,
    /// Can't assign a drive letter or mount point.
    ///
    /// This probably means that the mount point is already used by another volume.
    VfsMount = 2048 + 6,
    /// The mount point is invalid.
    VfsMountPoint = 2048 + 7,
    /// The Dokan version that this wrapper is targeting is incompatible with the loaded Dokan
    /// library.
    VfsVersion = 2048 + 8,

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

impl ToErrorCode for MirrorError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
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
            Self::InvalidArgument => ErrorCode::InvalidArgument,
            Self::Transport(error) => error.to_error_code(),
            Self::CreateRepository(_) => ErrorCode::Other,
        }
    }
}

impl ToErrorCode for OpenError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Config(_) => ErrorCode::Config,
            Self::Repository(error) => error.to_error_code(),
        }
    }
}

impl ToErrorCode for MountError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::FailedToParseMountPoint => ErrorCode::VfsFailedToParseMountPoint,
            Self::UnsupportedOs => ErrorCode::VfsUnsupportedOs,
            Self::Start => ErrorCode::VfsStart,
            Self::General => ErrorCode::VfsGeneral,
            Self::DriveLetter => ErrorCode::VfsDriveLetter,
            Self::DriverInstall => ErrorCode::VfsDriverInstall,
            Self::Mount => ErrorCode::VfsMount,
            Self::MountPoint => ErrorCode::VfsMountPoint,
            Self::Version => ErrorCode::VfsVersion,
        }
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

use num_enum::{IntoPrimitive, TryFromPrimitive};
use ouisync_macros::api;
#[cfg(feature = "vfs")]
use ouisync_vfs::MountError;
use serde::{Deserialize, Serialize};

use crate::{
    error::Error,
    protocol::ProtocolError,
    repository::FindError,
    transport::{ClientError, ReadError, ValidateError, WriteError},
};

use super::UnexpectedResponse;

#[derive(
    Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, IntoPrimitive, TryFromPrimitive,
)]
#[repr(u16)]
#[serde(into = "u16", try_from = "u16")]
#[api]
pub enum ErrorCode {
    /// No error
    Ok = 0,

    // # Generic errors
    /// Insuficient permission to perform the intended operation
    PermissionDenied = 1,
    /// Invalid input parameter
    InvalidInput = 2,
    /// Invalid data (e.g., malformed incoming message, config file, etc...)
    InvalidData = 3,
    /// Entry already exists
    AlreadyExists = 4,
    /// Entry not found
    NotFound = 5,
    /// Multiple matching entries found
    Ambiguous = 6,
    /// The indended operation is not supported
    Unsupported = 8,
    /// The operation was interrupted
    Interrupted = 9,

    // # Network errors
    /// Failed to establish connection to the server
    ConnectionRefused = 1024 + 1,
    /// Connection aborted by the server
    ConnectionAborted = 1024 + 2,
    /// Failed to send or receive message
    TransportError = 1024 + 3,
    /// Listener failed to bind to the specified address
    ListenerBindError = 1024 + 4,
    /// Listener failed to accept client connection
    ListenerAcceptError = 1024 + 5,

    // # Repository errors
    /// Operation on the internal repository store failed
    StoreError = 2048 + 1,
    /// Entry was expected to not be a directory but it is
    IsDirectory = 2048 + 2,
    /// Entry was expected to be a directory but it isn't
    NotDirectory = 2048 + 3,
    /// Directory was expected to be empty but it isn't
    DirectoryNotEmpty = 2048 + 4,
    /// File or directory is busy
    ResourceBusy = 2048 + 5,

    // # Service errors
    /// Failed to initialize runtime
    RuntimeInitializeError = 4096 + 1,
    /// Failed to read from or write into the config file
    ConfigError = 4096 + 3,
    /// TLS certificated not found
    TlsCertificatesNotFound = 4096 + 4,
    /// TLS certificates failed to load
    TlsCertificatesInvalid = 4096 + 5,
    /// TLS keys not found
    TlsKeysNotFound = 4096 + 6,
    /// Failed to create TLS config
    TlsConfigError = 4096 + 7,
    #[cfg(feature = "vfs")]
    /// Failed to install virtual filesystem driver
    VfsDriverInstallError = 4096 + 8,
    #[cfg(feature = "vfs")]
    /// Unspecified virtual filesystem error
    VfsOtherError = 4096 + 9,
    /// Another instance of the service is already running
    ServiceAlreadyRunning = 4096 + 10,
    /// Store directory is not specified
    StoreDirUnspecified = 4096 + 11,
    /// Mount directory is not specified
    MountDirUnspecified = 4096 + 12,
    #[cfg(not(feature = "vfs"))]
    /// Ouisync compiled without VFS
    NoVFS = 4096 + 13,

    /// Unspecified error
    Other = 65535,
}

pub(crate) trait ToErrorCode {
    fn to_error_code(&self) -> ErrorCode;
}

impl ToErrorCode for Error {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::AlreadyExists => ErrorCode::AlreadyExists,
            Self::Config(_) => ErrorCode::ConfigError,
            #[cfg(feature = "vfs")]
            Self::CreateMounter(error) => error.to_error_code(),
            #[cfg(not(feature = "vfs"))]
            Self::NoVFS => ErrorCode::NoVFS,
            Self::InitializeRuntime(_) => ErrorCode::RuntimeInitializeError,
            Self::InvalidArgument => ErrorCode::InvalidInput,
            Self::Io(_) => ErrorCode::Other,
            Self::NotFound => ErrorCode::NotFound,
            Self::Ambiguous => ErrorCode::Ambiguous,
            Self::Busy => ErrorCode::ResourceBusy,
            Self::OperationInterrupted => ErrorCode::Interrupted,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::Repository(error) => error.to_error_code(),
            Self::Store(_) => ErrorCode::StoreError,
            Self::StoreDirUnspecified => ErrorCode::StoreDirUnspecified,
            Self::MountDirUnspecified => ErrorCode::MountDirUnspecified,
            Self::TlsCertificatesNotFound => ErrorCode::TlsCertificatesNotFound,
            Self::TlsCertificatesInvalid(_) => ErrorCode::TlsCertificatesInvalid,
            Self::TlsConfig(_) => ErrorCode::TlsConfigError,
            Self::TlsKeysNotFound => ErrorCode::TlsKeysNotFound,
            Self::ServiceAlreadyRunning => ErrorCode::ServiceAlreadyRunning,
            Self::Bind(_) => ErrorCode::ListenerBindError,
            Self::Accept(_) => ErrorCode::ListenerAcceptError,
            Self::Client(error) => error.to_error_code(),
        }
    }
}

impl ToErrorCode for FindError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::NotFound => ErrorCode::NotFound,
            Self::Ambiguous => ErrorCode::Ambiguous,
        }
    }
}

impl ToErrorCode for ProtocolError {
    fn to_error_code(&self) -> ErrorCode {
        self.code()
    }
}

impl ToErrorCode for ClientError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Authentication => ErrorCode::PermissionDenied,
            Self::Connect(_) => ErrorCode::ConnectionRefused,
            Self::Disconnected => ErrorCode::ConnectionAborted,
            Self::InvalidArgument => ErrorCode::InvalidInput,
            Self::InvalidEndpoint(_) => ErrorCode::InvalidInput,
            Self::Io(_) => ErrorCode::Other,
            Self::Read(ReadError::Receive(_)) => ErrorCode::TransportError,
            Self::Read(ReadError::Decode(_)) => ErrorCode::InvalidData,
            Self::Read(ReadError::Validate(_, ValidateError::PermissionDenied)) => {
                ErrorCode::PermissionDenied
            }
            Self::Response(error) => error.to_error_code(),
            Self::UnexpectedResponse => ErrorCode::InvalidData,
            Self::Write(WriteError::Send(_)) => ErrorCode::TransportError,
            Self::Write(WriteError::Encode(_)) => ErrorCode::InvalidInput,
        }
    }
}

impl ToErrorCode for ValidateError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::PermissionDenied => ErrorCode::PermissionDenied,
        }
    }
}

impl ToErrorCode for UnexpectedResponse {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::InvalidData
    }
}

#[cfg(feature = "vfs")]
impl ToErrorCode for MountError {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::DriverInstall => ErrorCode::VfsDriverInstallError,
            Self::InvalidMountPoint | Self::Unsupported | Self::Backend(_) => {
                ErrorCode::VfsOtherError
            }
        }
    }
}

impl ToErrorCode for ouisync::Error {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Db(_) => ErrorCode::StoreError,
            Self::Store(_) => ErrorCode::StoreError,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::MalformedData => ErrorCode::InvalidData,
            Self::InvalidArgument => ErrorCode::InvalidInput,
            Self::MalformedDirectory => ErrorCode::InvalidData,
            Self::EntryExists => ErrorCode::AlreadyExists,
            Self::EntryNotFound => ErrorCode::NotFound,
            Self::AmbiguousEntry => ErrorCode::Ambiguous,
            Self::EntryIsFile => ErrorCode::NotDirectory,
            Self::EntryIsDirectory => ErrorCode::IsDirectory,
            Self::NonUtf8FileName => ErrorCode::InvalidInput,
            Self::OffsetOutOfRange => ErrorCode::InvalidInput,
            Self::DirectoryNotEmpty => ErrorCode::DirectoryNotEmpty,
            Self::OperationNotSupported => ErrorCode::Unsupported,
            Self::Writer(_) => ErrorCode::Other,
            Self::Locked => ErrorCode::ResourceBusy,
            Self::StorageVersionMismatch => ErrorCode::InvalidData,
        }
    }
}

impl ToErrorCode for rmp_serde::decode::Error {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::InvalidData
    }
}

impl<T, E> ToErrorCode for Result<T, E>
where
    E: ToErrorCode,
{
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Ok(_) => ErrorCode::Ok,
            Err(error) => error.to_error_code(),
        }
    }
}

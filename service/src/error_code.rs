use num_enum::{IntoPrimitive, TryFromPrimitive};
use serde::{Deserialize, Serialize};

use crate::{
    error::Error,
    protocol::ProtocolError,
    repository::FindError,
    transport::{ClientError, ReadError, ValidateError, WriteError},
};

#[derive(
    Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize, IntoPrimitive, TryFromPrimitive,
)]
#[repr(u16)]
#[serde(into = "u16", try_from = "u16")]
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

    // # Network errors
    /// Failed to establish connection to the server
    ConnectionRefused = 1024 + 1,
    /// Connection aborted by the server
    ConnectionAborted = 1024 + 2,
    /// Failed to send or receive message
    TransportError = 1024 + 3,
    /// Listener failed to bind to the specified address
    ListenerBind = 1024 + 4,
    /// Listener failed to accept client connection
    ListenerAccept = 1024 + 5,

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
    /// TLS certificated not found
    TlsCertificatesNotFound = 4096 + 1,
    /// TLS certificates failed to load
    TlsCertificatesInvalid = 4096 + 2,
    /// TLS keys not found
    TlsKeysNotFound = 4096 + 3,
    /// Failed to create TLS config
    TlsConfig = 4096 + 4,
    /// Failed to read from or write into the config file
    Config = 4096 + 5,
    /// Failed to create mounter
    CreateMounter = 4096 + 6,

    /// Unspecified error
    Other = 65535,
}

pub(crate) trait ToErrorCode {
    fn to_error_code(&self) -> ErrorCode;
}

impl ToErrorCode for Error {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Config(_) => ErrorCode::Config,
            Self::CreateMounter(_) => ErrorCode::CreateMounter,
            Self::Io(_) => ErrorCode::Other,
            Self::OperationNotSupported => ErrorCode::Unsupported,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::Repository(error) => error.to_error_code(),
            Self::RepositoryExists => ErrorCode::AlreadyExists,
            Self::RepositoryNotFound => ErrorCode::NotFound,
            Self::RepositorySyncDisabled => ErrorCode::Unsupported,
            Self::Store(_) => todo!(),
            Self::TlsCertificatesNotFound => ErrorCode::TlsCertificatesNotFound,
            Self::TlsCertificatesInvalid(_) => ErrorCode::TlsCertificatesInvalid,
            Self::TlsConfig(_) => ErrorCode::TlsConfig,
            Self::TlsKeysNotFound => ErrorCode::TlsKeysNotFound,
            Self::Bind(_) => ErrorCode::ListenerBind,
            Self::Accept(_) => ErrorCode::ListenerAccept,
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
            Self::Connect(_) => ErrorCode::ConnectionRefused,
            Self::Disconnected => ErrorCode::ConnectionAborted,
            Self::InvalidArgument => ErrorCode::InvalidInput,
            Self::InvalidSocketAddr => ErrorCode::InvalidInput,
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

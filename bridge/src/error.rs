use serde::Serialize;
use std::io;
use thiserror::Error;

/// A specialized `Result` type for convenience.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("library error")]
    Library(#[from] ouisync_lib::Error),
    #[error("failed to initialize logger")]
    InitializeLogger(#[source] io::Error),
    #[error("failed to initialize runtime")]
    InitializeRuntime(#[source] io::Error),
    #[error("failed to bind listener")]
    Bind(#[source] io::Error),
    #[error("request is malformed")]
    MalformedRequest(#[source] rmp_serde::decode::Error),
    #[error("argument is not valid")]
    InvalidArgument,
}

impl Error {
    pub fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Library(error) => {
                use ouisync_lib::Error::*;

                match error {
                    Db(_) => ErrorCode::Db,
                    DeviceIdConfig(_) => ErrorCode::DeviceIdConfig,
                    PermissionDenied => ErrorCode::PermissionDenied,
                    MalformedData | MalformedDirectory => ErrorCode::MalformedData,
                    EntryExists => ErrorCode::EntryExists,
                    EntryNotFound => ErrorCode::EntryNotFound,
                    AmbiguousEntry => ErrorCode::AmbiguousEntry,
                    DirectoryNotEmpty => ErrorCode::DirectoryNotEmpty,
                    OperationNotSupported | ConcurrentWriteNotSupported => {
                        ErrorCode::OperationNotSupported
                    }
                    NonUtf8FileName | OffsetOutOfRange => ErrorCode::InvalidArgument,
                    StorageVersionMismatch => ErrorCode::StorageVersionMismatch,
                    BlockNotFound(_) | BlockNotReferenced | WrongBlockLength(_) | EntryIsFile
                    | EntryIsDirectory | Writer(_) | RequestTimeout => ErrorCode::Other,
                }
            }
            Self::InitializeLogger(_) | Self::InitializeRuntime(_) | Self::Bind(_) => {
                ErrorCode::Other
            }
            Self::MalformedRequest(_) => ErrorCode::MalformedRequest,
            Self::InvalidArgument => ErrorCode::InvalidArgument,
        }
    }
}

#[derive(Copy, Clone, Serialize)]
#[repr(u16)]
#[serde(into = "u16")]
pub enum ErrorCode {
    /// No error
    Ok = 0,
    /// Database error
    Db = 1,
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
    /// Failed to read from or write into the device ID config file
    DeviceIdConfig = 10,
    /// Argument passed to a function is not valid
    InvalidArgument = 11,
    /// Interface request is malformed
    MalformedRequest = 12,
    /// Storage format version mismatch
    StorageVersionMismatch = 13,
    /// Unspecified error
    Other = 65535,
}

impl From<ErrorCode> for u16 {
    fn from(error_code: ErrorCode) -> u16 {
        error_code as u16
    }
}

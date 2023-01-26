use ouisync_lib::Error;
use serde::Serialize;
use std::io;

#[derive(Serialize)]
#[repr(C)]
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
    /// Unspecified error
    Other = 65536,
}

pub(crate) trait ToErrorCode {
    fn to_error_code(&self) -> ErrorCode;
}

impl ToErrorCode for Error {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::Db(_) => ErrorCode::Db,
            Self::DeviceIdConfig(_) => ErrorCode::DeviceIdConfig,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::MalformedData | Self::MalformedDirectory => ErrorCode::MalformedData,
            Self::EntryExists => ErrorCode::EntryExists,
            Self::EntryNotFound => ErrorCode::EntryNotFound,
            Self::AmbiguousEntry => ErrorCode::AmbiguousEntry,
            Self::DirectoryNotEmpty => ErrorCode::DirectoryNotEmpty,
            Self::OperationNotSupported | Self::ConcurrentWriteNotSupported => ErrorCode::OperationNotSupported,
            Self::BlockNotFound(_)
            | Self::BlockNotReferenced
            | Self::WrongBlockLength(_)
            | Self::EntryIsFile
            | Self::EntryIsDirectory
            | Self::NonUtf8FileName
            | Self::OffsetOutOfRange
            | Self::InitializeLogger(_)
            | Self::InitializeRuntime(_)
            | Self::Interface(_)
            | Self::Writer(_)
            | Self::RequestTimeout
            // TODO: add separate code for `StorageVersionMismatch`
            | Self::StorageVersionMismatch => ErrorCode::Other,
        }
    }
}

impl ToErrorCode for io::Error {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::Other
    }
}

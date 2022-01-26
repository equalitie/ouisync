use crate::error::Error;
use std::io;

#[repr(u32)]
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
    /// Network error
    Network = 9,
    /// Unspecified error
    Other = u32::MAX,
}

pub(crate) trait ToErrorCode {
    fn to_error_code(&self) -> ErrorCode;
}

impl ToErrorCode for Error {
    fn to_error_code(&self) -> ErrorCode {
        match self {
            Self::CreateDbDirectory(_)
            | Self::ConnectToDb(_)
            | Self::CreateDbSchema(_)
            | Self::QueryDb(_) => ErrorCode::Db,
            Self::PermissionDenied => ErrorCode::PermissionDenied,
            Self::MalformedData | Self::MalformedDirectory(_) => ErrorCode::MalformedData,
            Self::EntryExists => ErrorCode::EntryExists,
            Self::EntryNotFound => ErrorCode::EntryNotFound,
            Self::AmbiguousEntry => ErrorCode::AmbiguousEntry,
            Self::DirectoryNotEmpty => ErrorCode::DirectoryNotEmpty,
            Self::OperationNotSupported => ErrorCode::OperationNotSupported,
            Self::Network(_) => ErrorCode::Network,
            Self::BlockNotFound(_)
            | Self::BlockNotReferenced
            | Self::WrongBlockLength(_)
            | Self::EntryIsFile
            | Self::EntryIsDirectory
            | Self::NonUtf8FileName
            | Self::OffsetOutOfRange
            | Self::WriterSet(_) => ErrorCode::Other,
        }
    }
}

impl ToErrorCode for io::Error {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::Other
    }
}

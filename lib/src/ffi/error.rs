use crate::error::Error;
use std::io;

#[repr(u32)]
pub enum ErrorCode {
    /// No error
    Ok = 0,
    Db = 1,
    PermissionDenied = 2,
    MalformedData = 3,
    BlockNotFound = 4,
    // BlockExists,
    // #[error("block is not referenced by the index")]
    // BlockNotReferenced,
    // #[error("block has wrong length (expected: {}, actual: {0})", BLOCK_SIZE)]
    // WrongBlockLength(usize),
    // #[error("not a directory or directory malformed")]
    // MalformedDirectory(#[source] bincode::Error),
    // #[error("entry already exists")]
    // EntryExists,
    // #[error("entry not found")]
    // EntryNotFound,
    // #[error("entry has multiple concurrent versions")]
    // AmbiguousEntry,
    // #[error("entry is a file")]
    // EntryIsFile,
    // #[error("entry is a directory")]
    // EntryIsDirectory,
    // #[error("File name is not a valid UTF-8 string")]
    // NonUtf8FileName,
    // #[error("offset is out of range")]
    // OffsetOutOfRange,
    // #[error("directory is not empty")]
    // DirectoryNotEmpty,
    // #[error("operation is not supported")]
    // OperationNotSupported,
    // #[error("network error")]
    // Network(#[source] io::Error),
    // #[error("writer set error")]
    // WriterSet(crate::writer_set::error::Error),
}

pub(crate) trait ToErrorCode {
    fn to_error_code(&self) -> ErrorCode;
}

impl ToErrorCode for Error {}

impl ToErrorCode for io::Error {}

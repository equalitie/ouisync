use crate::{
    block::{BlockId, BLOCK_SIZE},
    db, store,
};
use std::{array::TryFromSliceError, fmt, io};
use thiserror::Error;

/// A specialized `Result` type for convenience.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    // TODO: remove
    #[error("database error")]
    Db(#[from] db::Error),
    #[error("store error")]
    Store(#[from] store::Error),
    #[error("permission denied")]
    PermissionDenied,
    // TODO: remove
    #[error("data is malformed")]
    MalformedData,
    // TODO: remove
    #[error("block not found: {0}")]
    BlockNotFound(BlockId),
    // TODO: remove
    #[error("block has wrong length (expected: {}, actual: {0})", BLOCK_SIZE)]
    WrongBlockLength(usize),
    #[error("not a directory or directory malformed")]
    MalformedDirectory,
    #[error("entry already exists")]
    EntryExists,
    #[error("entry not found")]
    EntryNotFound,
    #[error("ambiguous entry")]
    AmbiguousEntry,
    #[error("entry is a file")]
    EntryIsFile,
    #[error("entry is a directory")]
    EntryIsDirectory,
    #[error("File name is not a valid UTF-8 string")]
    NonUtf8FileName,
    #[error("offset is out of range")]
    OffsetOutOfRange,
    #[error("directory is not empty")]
    DirectoryNotEmpty,
    #[error("operation is not supported")]
    OperationNotSupported,
    #[error("failed to write into writer")]
    Writer(#[source] io::Error),
    #[error("storage version mismatch")]
    StorageVersionMismatch,
    #[error("file or directory is locked")]
    Locked,
}

impl Error {
    /// Returns an object that implements `Display` which prints this error together with its whole
    /// causal chain.
    pub fn verbose(&self) -> Verbose {
        Verbose(self)
    }
}

impl From<TryFromSliceError> for Error {
    fn from(_: TryFromSliceError) -> Self {
        Self::MalformedData
    }
}

impl From<sqlx::Error> for Error {
    fn from(src: sqlx::Error) -> Self {
        Self::Db(src.into())
    }
}

pub struct Verbose<'a>(&'a Error);

impl fmt::Display for Verbose<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use std::error::Error;

        writeln!(f, "{}", self.0)?;

        let mut current = self.0 as &dyn Error;

        while let Some(source) = current.source() {
            writeln!(f, "    caused by: {}", source)?;
            current = source;
        }

        Ok(())
    }
}

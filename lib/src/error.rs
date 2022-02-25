use crate::block::{BlockId, BLOCK_SIZE};
use std::{array::TryFromSliceError, fmt, io};
use thiserror::Error;

/// A specialized `Result` type for convenience.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create database directory")]
    CreateDbDirectory(#[source] io::Error),
    #[error("failed to establish database connection")]
    ConnectToDb(#[source] sqlx::Error),
    #[error("failed to create database schema")]
    CreateDbSchema(#[source] sqlx::Error),
    #[error("failed to execute database query")]
    QueryDb(#[from] sqlx::Error),
    #[error("failed to read from or write into the device ID config file")]
    DeviceIdConfig(#[source] io::Error),
    #[error("permission denied")]
    PermissionDenied,
    #[error("data is malformed")]
    MalformedData,
    #[error("block not found: {0}")]
    BlockNotFound(BlockId),
    #[error("block is not referenced by the index")]
    BlockNotReferenced,
    #[error("block has wrong length (expected: {}, actual: {0})", BLOCK_SIZE)]
    WrongBlockLength(usize),
    #[error("not a directory or directory malformed")]
    MalformedDirectory(#[source] bincode::Error),
    #[error("entry already exists")]
    EntryExists,
    #[error("entry not found")]
    EntryNotFound,
    #[error("entry has multiple concurrent versions")]
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
    #[error("network error")]
    Network(#[source] io::Error),
    #[error("failed to initialize logger")]
    InitializeLogger(#[source] io::Error),
    #[error("failed to initialize runtime")]
    InitializeRuntime(#[source] io::Error),
    #[error("failed to write into writer")]
    Writer(#[source] io::Error),
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
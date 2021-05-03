use crate::{
    block::{BlockId, BLOCK_SIZE},
    crypto::aead,
};
use std::{array::TryFromSliceError, io};
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
    #[error("data is malformed")]
    MalformedData,
    #[error("block not found: {0}")]
    BlockNotFound(BlockId),
    #[error("block id not found")]
    BlockIdNotFound,
    #[error("block has wrong length (expected: {}, actual: {0})", BLOCK_SIZE)]
    WrongBlockLength(usize),
    #[error("encryption / decryption failed")]
    Crypto,
    #[error("not a directory or directory malformed")]
    MalformedDirectory(#[source] bincode::Error),
    #[error("entry already exists")]
    EntryExists,
    #[error("entry not found")]
    EntryNotFound,
    #[error("entry is not a directory")]
    EntryNotDirectory,
    #[error("directory entry offset is out of range")]
    WrongDirectoryEntryOffset,
}

impl From<TryFromSliceError> for Error {
    fn from(_: TryFromSliceError) -> Self {
        Self::MalformedData
    }
}

impl From<aead::Error> for Error {
    fn from(_: aead::Error) -> Self {
        Self::Crypto
    }
}

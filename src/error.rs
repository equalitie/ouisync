use crate::block::{BlockId, BLOCK_SIZE};
use std::{array::TryFromSliceError, io};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create database directory")]
    CreateDbDirectory(#[source] io::Error),
    #[error("failed to establish database connection")]
    ConnectToDb(#[source] sqlx::Error),
    #[error("failed to create database schema")]
    CreateDbSchema(#[source] sqlx::Error),
    #[error("failed to execute database query")]
    QueryDb(#[source] sqlx::Error),
    #[error("data is malformed")]
    MalformedData,
    #[error("block not found: {0}")]
    BlockNotFound(BlockId),
    #[error("block id not found")]
    BlockIdNotFound,
    #[error("block has wrong length (expected: {}, actual: {0})", BLOCK_SIZE)]
    WrongBlockLength(usize),
}

impl From<TryFromSliceError> for Error {
    fn from(_: TryFromSliceError) -> Self {
        Self::MalformedData
    }
}

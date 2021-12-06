use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database is already set up")]
    WriterSetIsAlreadySetUp,
    #[error("origin entry does not have a valid signature")]
    InvalidOriginEntry,
    #[error("an entry does not have a valid signature")]
    InvalidEntry,
    #[error("origin not found in the writer set")]
    OriginNotFound,
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("SQL error")]
    Sql(#[from] rusqlite::Error),
    // TODO: dummy error variant - remove
    #[error("invalid state")]
    InvalidState,
}

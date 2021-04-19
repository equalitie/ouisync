use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("SQL error")]
    Sql(#[from] sqlx::Error),
}

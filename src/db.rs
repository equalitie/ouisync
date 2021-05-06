use crate::{
    block,
    error::{Error, Result},
    index, this_replica,
};
use sqlx::{
    sqlite::{Sqlite, SqliteConnectOptions},
    SqlitePool,
};
use std::path::Path;
use tokio::fs;

/// Database connection pool.
pub type Pool = SqlitePool;

/// Database transaction
pub type Transaction = sqlx::Transaction<'static, Sqlite>;

/// Creates the database unless it already exsits and establish a connection to it.
pub async fn init(path: impl AsRef<Path>) -> Result<Pool> {
    if let Some(dir) = path.as_ref().parent() {
        fs::create_dir_all(dir)
            .await
            .map_err(Error::CreateDbDirectory)?;
    }

    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
    .map_err(Error::ConnectToDb)?;

    create_schema(&pool).await?;

    Ok(pool)
}

// Create the database schema
pub async fn create_schema(pool: &Pool) -> Result<()> {
    block::init(&pool).await?;
    index::init(&pool).await?;
    this_replica::init(&pool).await?;
    Ok(())
}

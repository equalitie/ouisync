use crate::{
    block,
    error::{Error, Result},
    index, this_replica,
};
use sqlx::{
    sqlite::{Sqlite, SqliteConnectOptions},
    SqlitePool,
};
use std::path::PathBuf;
use tokio::fs;

/// Database connection pool.
pub type Pool = SqlitePool;

/// Database transaction
pub type Transaction = sqlx::Transaction<'static, Sqlite>;

/// Database connection
pub type Connection = sqlx::pool::PoolConnection<Sqlite>;

/// Database store.
#[derive(Debug)]
pub enum Store {
    /// Database stored on the filesystem.
    File(PathBuf),
    /// Temporary database stored in memory.
    Memory,
}

/// Creates the database unless it already exsits and establish a connection to it.
pub async fn init(store: Store) -> Result<Pool> {
    let pool = match store {
        Store::File(path) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)
                    .await
                    .map_err(Error::CreateDbDirectory)?;
            }

            Pool::connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await
        }
        Store::Memory => Pool::connect(":memory:").await,
    }
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

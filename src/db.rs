use crate::{
    block,
    error::{Error, Result},
    index, this_replica,
};
use sqlx::{
    pool::PoolOptions,
    sqlite::{Sqlite, SqliteConnectOptions},
    SqlitePool,
};
use std::{path::PathBuf, str::FromStr};
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
    let options = match store {
        Store::File(path) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)
                    .await
                    .map_err(Error::CreateDbDirectory)?;
            }

            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
        }
        Store::Memory => SqliteConnectOptions::from_str(":memory:").expect("invalid db uri"),
    };

    let pool = PoolOptions::new()
        // HACK: Using only one connection turns the pool effectively into a mutex over a single
        // connection. This is a heavy-handed fix that prevents the "table is locked" errors that
        // sometimes happen when multiple tasks try to access the same table and at least one of
        // them mutably. The downside is that this means only one task can access the database at
        // any given time which might affect performance.
        // TODO: find a more fine-grained way to solve this issue.
        .max_connections(1)
        .connect_with(options)
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

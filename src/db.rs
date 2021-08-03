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
pub type Transaction<'a> = sqlx::Transaction<'a, Sqlite>;

/// Database connection
pub type Connection = sqlx::pool::PoolConnection<Sqlite>;

/// This trait allows to write functions that work with any of `Pool`, `Connection` or
/// `Transaction`. It's an alias for `sqlx::Executor<Database = Sqlite>` for convenience.
pub trait Executor<'a>: sqlx::Executor<'a, Database = Sqlite> {}
impl<'a, T> Executor<'a> for T where T: sqlx::Executor<'a, Database = Sqlite> {}

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
    block::init(pool).await?;
    index::init(pool).await?;
    this_replica::init(pool).await?;
    Ok(())
}

// Explicit cast from `i64` to `u64` to work around the lack of native `u64` support in the sqlx
// crate.
pub const fn decode_u64(i: i64) -> u64 {
    i as u64
}

// Explicit cast from `u64` to `i64` to work around the lack of native `u64` support in the sqlx
// crate.
pub const fn encode_u64(u: u64) -> i64 {
    u as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    // Check the casts are lossless

    #[test]
    fn decode_u64_sanity_check() {
        // [0i64,     i64::MAX] -> [0u64,             u64::MAX / 2]
        // [i64::MIN,    -1i64] -> [u64::MAX / 2 + 1,     u64::MAX]

        assert_eq!(decode_u64(0), 0);
        assert_eq!(decode_u64(1), 1);
        assert_eq!(decode_u64(-1), u64::MAX);
        assert_eq!(decode_u64(i64::MIN), u64::MAX / 2 + 1);
        assert_eq!(decode_u64(i64::MAX), u64::MAX / 2);
    }

    #[test]
    fn encode_u64_sanity_check() {
        assert_eq!(encode_u64(0), 0);
        assert_eq!(encode_u64(1), 1);
        assert_eq!(encode_u64(u64::MAX / 2), i64::MAX);
        assert_eq!(encode_u64(u64::MAX / 2 + 1), i64::MIN);
        assert_eq!(encode_u64(u64::MAX), -1);
    }
}

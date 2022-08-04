mod migrations;

use sqlx::{
    sqlite::{
        Sqlite, SqliteConnectOptions, SqliteConnection, SqlitePoolOptions,
        SqliteTransactionBehavior, SqliteTransactionOptions,
    },
    Connection as _, Row, SqlitePool,
};
use std::{io, path::Path};
#[cfg(test)]
use tempfile::TempDir;
use thiserror::Error;
use tokio::fs;

/// Database connection pool.
#[derive(Clone)]
pub struct Pool {
    inner: SqlitePool,
}

impl Pool {
    fn new(inner: SqlitePool) -> Self {
        Self { inner }
    }

    pub async fn acquire(&self) -> Result<PoolConnection, sqlx::Error> {
        self.inner.acquire().await
    }

    pub async fn begin(&self) -> Result<PoolTransaction, sqlx::Error> {
        // NOTE: deferred transactions are prone to deadlock. Create immediate transaction by
        // default.
        self.inner
            .begin_with(
                SqliteTransactionOptions::default().behavior(SqliteTransactionBehavior::Immediate),
            )
            .await
    }

    pub(crate) async fn close(&self) {
        self.inner.close().await
    }
}

/// Database connection.
pub type Connection = SqliteConnection;

pub(crate) type PoolConnection = sqlx::pool::PoolConnection<Sqlite>;

/// Database transaction
pub type Transaction<'a> = sqlx::Transaction<'a, Sqlite>;

/// Database transaction obtained from `Pool::begin`.
pub(crate) type PoolTransaction = sqlx::Transaction<'static, Sqlite>;

pub(crate) async fn begin_immediate(conn: &mut Connection) -> Result<Transaction<'_>, sqlx::Error> {
    conn.begin_with(
        SqliteTransactionOptions::default().behavior(SqliteTransactionBehavior::Immediate),
    )
    .await
}

/// Creates a new database and opens a connection to it.
pub(crate) async fn create(path: impl AsRef<Path>) -> Result<Pool, Error> {
    let path = path.as_ref();

    if fs::metadata(path).await.is_ok() {
        return Err(Error::Exists);
    }

    create_directory(path).await?;

    let connect_options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    let pool = create_pool(connect_options).await?;

    let mut conn = pool.acquire().await?;
    migrations::run(&mut conn).await?;

    Ok(pool)
}

/// Creates a new database in a temporary directory. Useful for tests.
#[cfg(test)]
pub(crate) async fn create_temp() -> Result<(TempDir, Pool), Error> {
    let temp_dir = TempDir::new().map_err(Error::CreateDirectory)?;
    let pool = create(temp_dir.path().join("temp.db")).await?;

    Ok((temp_dir, pool))
}

/// Opens a connection to the specified database. Fails if the db doesn't exist.
pub(crate) async fn open(path: impl AsRef<Path>) -> Result<Pool, Error> {
    let connect_options = SqliteConnectOptions::new().filename(path);
    let pool = create_pool(connect_options).await?;

    let mut conn = pool.acquire().await?;
    migrations::run(&mut conn).await?;

    Ok(pool)
}

async fn create_directory(path: &Path) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .await
            .map_err(Error::CreateDirectory)?
    }

    Ok(())
}

async fn create_pool(connect_options: SqliteConnectOptions) -> Result<Pool, Error> {
    // TODO: consider enabling shared cache to improve performance. Currently it doesn't work
    // properly (concurrent access is triggering "table locked" errors) possibly due to buggy
    // implementation of the unlock notify in sqlx
    // let connect_options = connect_options.shared_cache(true);

    SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(connect_options)
        .await
        .map(Pool::new)
        .map_err(Error::Open)
}

// Explicit cast from `i64` to `u64` to work around the lack of native `u64` support in the sqlx
// crate.
pub(crate) const fn decode_u64(i: i64) -> u64 {
    i as u64
}

// Explicit cast from `u64` to `i64` to work around the lack of native `u64` support in the sqlx
// crate.
pub(crate) const fn encode_u64(u: u64) -> i64 {
    u as i64
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create database directory")]
    CreateDirectory(#[source] io::Error),
    #[error("database already exists")]
    Exists,
    #[error("failed to open database")]
    Open(#[source] sqlx::Error),
    #[error("failed to execute database query")]
    Query(#[from] sqlx::Error),
}

async fn get_pragma(conn: &mut Connection, name: &str) -> Result<u32, Error> {
    Ok(sqlx::query(&format!("PRAGMA {}", name))
        .fetch_one(&mut *conn)
        .await?
        .get(0))
}

async fn set_pragma(conn: &mut Connection, name: &str, value: u32) -> Result<(), Error> {
    // `bind` doesn't seem to be supported for setting PRAGMAs...
    sqlx::query(&format!("PRAGMA {} = {}", name, value))
        .execute(&mut *conn)
        .await?;

    Ok(())
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

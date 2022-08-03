mod migrations;

use crate::deadlock::{DeadlockGuard, DeadlockTracker};
use sqlx::{
    sqlite::{Sqlite, SqliteConnectOptions, SqliteConnection, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::{
    future::Future,
    io,
    ops::{Deref, DerefMut},
    path::Path,
};
#[cfg(test)]
use tempfile::TempDir;
use thiserror::Error;
use tokio::fs;

/// Database connection pool.
#[derive(Clone)]
pub struct Pool {
    inner: SqlitePool,
    deadlock_tracker: DeadlockTracker,
}

impl Pool {
    fn new(inner: SqlitePool) -> Self {
        Self {
            inner,
            deadlock_tracker: DeadlockTracker::new(),
        }
    }

    #[track_caller]
    pub fn acquire(&self) -> impl Future<Output = Result<PoolConnection, sqlx::Error>> {
        DeadlockGuard::try_wrap(self.inner.acquire(), self.deadlock_tracker.clone())
    }

    #[track_caller]
    pub(crate) fn begin(&self) -> impl Future<Output = Result<PoolTransaction, sqlx::Error>> + '_ {
        let future = DeadlockGuard::try_wrap(self.inner.begin(), self.deadlock_tracker.clone());
        async move { Ok(PoolTransaction(future.await?)) }
    }

    pub(crate) async fn close(&self) {
        self.inner.close().await
    }
}

/// Database connection.
pub type Connection = SqliteConnection;

pub(crate) type PoolConnection = DeadlockGuard<sqlx::pool::PoolConnection<Sqlite>>;

/// Database transaction
pub(crate) type Transaction<'a> = sqlx::Transaction<'a, Sqlite>;

/// Database transaction obtained from `Pool::begin`.
pub(crate) struct PoolTransaction(DeadlockGuard<sqlx::Transaction<'static, Sqlite>>);

impl PoolTransaction {
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.0.into_inner().commit().await
    }
}

impl Deref for PoolTransaction {
    type Target = Transaction<'static>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl DerefMut for PoolTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut()
    }
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
    SqlitePoolOptions::new()
        // HACK: using only one connection to work around concurrency issues (`SQLITE_BUSY` errors)
        // TODO: find a way to reliable use multiple connections
        .max_connections(1)
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

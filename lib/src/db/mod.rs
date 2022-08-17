mod migrations;

use futures_util::Stream;
use ref_cast::RefCast;
use sqlx::{
    sqlite::{
        Sqlite, SqliteConnectOptions, SqliteConnection, SqlitePoolOptions,
        SqliteTransactionBehavior,
    },
    Connection as _, Database, Either, Execute, Executor, Row, SqlitePool,
};
use std::{
    future::Future,
    io,
    ops::{Deref, DerefMut},
    path::Path,
    pin::Pin,
};
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
        self.inner.acquire().await.map(PoolConnection)
    }

    pub async fn begin(&self) -> Result<Transaction<'static>, sqlx::Error> {
        // NOTE: deferred transactions are prone to deadlocks. Create immediate transaction by
        // default.
        self.inner
            .begin_with(SqliteTransactionBehavior::Immediate.into())
            .await
            .map(Transaction)
    }

    pub(crate) async fn close(&self) {
        self.inner.close().await
    }
}

/// Database connection
#[derive(Debug, RefCast)]
#[repr(transparent)]
pub struct Connection(SqliteConnection);

impl Connection {
    pub async fn begin(&mut self) -> Result<Transaction<'_>, sqlx::Error> {
        // NOTE: deferred transactions are prone to deadlocks. Create immediate transaction by
        // default.
        self.0
            .begin_with(SqliteTransactionBehavior::Immediate.into())
            .await
            .map(Transaction)
    }
}

// Delegate the `Executor` impl to the inner type (what a trait!)
impl<'c> Executor<'c> for &'c mut Connection {
    type Database = Sqlite;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> Pin<
        Box<
            dyn Stream<
                    Item = Result<
                        Either<
                            <Self::Database as Database>::QueryResult,
                            <Self::Database as Database>::Row,
                        >,
                        sqlx::Error,
                    >,
                > + Send
                + 'e,
        >,
    >
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        self.0.fetch_many(query)
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<<Self::Database as Database>::Row>, sqlx::Error>>
                + Send
                + 'e,
        >,
    >
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        self.0.fetch_optional(query)
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [<Self::Database as Database>::TypeInfo],
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        <Self::Database as sqlx::database::HasStatement<'q>>::Statement,
                        sqlx::Error,
                    >,
                > + Send
                + 'e,
        >,
    >
    where
        'c: 'e,
    {
        self.0.prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> Pin<
        Box<dyn Future<Output = Result<sqlx::Describe<Self::Database>, sqlx::Error>> + Send + 'e>,
    >
    where
        'c: 'e,
    {
        self.0.describe(sql)
    }
}

/// Database connection from pool
pub struct PoolConnection(sqlx::pool::PoolConnection<Sqlite>);

impl Deref for PoolConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        Connection::ref_cast(self.0.deref())
    }
}

impl DerefMut for PoolConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Connection::ref_cast_mut(self.0.deref_mut())
    }
}

/// Database transaction
#[derive(Debug)]
pub struct Transaction<'a>(sqlx::Transaction<'a, Sqlite>);

impl Transaction<'_> {
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.0.commit().await
    }

    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.0.rollback().await
    }
}

impl Deref for Transaction<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        Connection::ref_cast(self.0.deref())
    }
}

impl DerefMut for Transaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Connection::ref_cast_mut(self.0.deref_mut())
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
    let connect_options = connect_options.pragma("recursive_triggers", "ON");

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

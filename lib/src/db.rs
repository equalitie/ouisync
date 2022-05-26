use crate::deadlock::{DeadlockGuard, DeadlockTracker};
use sqlx::{
    sqlite::{Sqlite, SqliteConnectOptions, SqliteConnection, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::{
    borrow::Cow,
    convert::Infallible,
    future::Future,
    io,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    str::FromStr,
};
use thiserror::Error;
use tokio::fs;

/// Number used as [application id](https://www.sqlite.org/pragma.html#pragma_application_id) to
/// distinguish ouisync database from any other sqlite database.
const MAGIC: u32 = 0x6f756973; // first 4 characters of "ouisync" converted to hex

/// Current storage format version.
const VERSION: u32 = 1;

/// Database connection pool.
#[derive(Clone)]
pub(crate) struct Pool {
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
    pub fn begin(&self) -> impl Future<Output = Result<PoolTransaction, sqlx::Error>> + '_ {
        let future = DeadlockGuard::try_wrap(self.inner.begin(), self.deadlock_tracker.clone());
        async move { Ok(PoolTransaction(future.await?)) }
    }

    pub async fn close(&self) {
        self.inner.close().await
    }
}

/// Database connection.
pub type Connection = SqliteConnection;

/// Pooled database connection
pub(crate) type PoolConnection = DeadlockGuard<sqlx::pool::PoolConnection<Sqlite>>;

/// Database transaction
pub type Transaction<'a> = sqlx::Transaction<'a, Sqlite>;

/// Database transaction obtained from `Pool::begin`.
pub struct PoolTransaction(DeadlockGuard<sqlx::Transaction<'static, Sqlite>>);

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

// URI of a memory-only db.
const MEMORY: &str = ":memory:";

/// Database store.
#[derive(Debug)]
pub enum Store {
    /// Permanent database stored in the specified file.
    Permanent(PathBuf),
    /// Temporary database wiped out on program termination.
    Temporary,
}

impl<'a> From<Cow<'a, Path>> for Store {
    fn from(path: Cow<'a, Path>) -> Self {
        if path.as_ref().to_str() == Some(MEMORY) {
            Self::Temporary
        } else {
            Self::Permanent(path.into_owned())
        }
    }
}

impl From<PathBuf> for Store {
    fn from(path: PathBuf) -> Self {
        Self::from(Cow::Owned(path))
    }
}

impl<'a> From<&'a Path> for Store {
    fn from(path: &'a Path) -> Self {
        Self::from(Cow::Borrowed(path))
    }
}

impl FromStr for Store {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(Path::new(s)))
    }
}

/// Creates a new database and opens a connection to it.
pub(crate) async fn create(store: &Store) -> Result<Pool, Error> {
    let connect_options = match store {
        Store::Permanent(path) => {
            create_directory(path).await?;

            if fs::metadata(path).await.is_ok() {
                return Err(Error::Exists);
            }

            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
        }
        Store::Temporary => SqliteConnectOptions::from_str(MEMORY)
            .unwrap()
            .shared_cache(false),
    };

    let pool = create_pool(connect_options).await?;

    let mut tx = pool.begin().await?;

    set_pragma(&mut *tx, "application_id", MAGIC).await?;
    set_pragma(&mut *tx, "user_version", VERSION).await?;

    sqlx::query(include_str!("../schema.sql"))
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(pool)
}

/// Opens a connection to the specified database. Fails if the db doesn't exist.
pub(crate) async fn open(store: &Store) -> Result<Pool, Error> {
    let connect_options = match store {
        Store::Permanent(path) => SqliteConnectOptions::new().filename(path),
        Store::Temporary => SqliteConnectOptions::from_str(MEMORY)
            .unwrap()
            .shared_cache(false),
    };

    let pool = create_pool(connect_options).await?;

    let mut conn = pool.acquire().await?;

    if get_pragma(&mut conn, "application_id").await? != MAGIC {
        return Err(Error::InvalidFormat);
    }

    // TODO: migrations
    if get_pragma(&mut conn, "user_version").await? != VERSION {
        return Err(Error::InvalidVersion);
    }

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
        // HACK: using only one connection to work around various issues:
        //
        // - For persistent database, this avoids `SQLITE_BUSY` errors.
        // - For memory database, this avoids having to use shared cache (which is necessary when
        //   using multiple connections to a memory database, but it's extremely prone to deadlocks)
        //
        // TODO: After some experimentation, it seems that using `SqliteSynchornous::Normal` might
        // fix those errors but it needs more testing. But even if it works, we should try to avoid
        // making the test and the production code diverge too much. This means that in order to
        // use multiple connections we would either have to stop using memory databases or we would
        // have to enable shared cache also for file databases. Both approaches have their
        // drawbacks.
        .max_connections(1)
        // Never reap connections because for memory database and without shared cache it would
        // also destroy the whole database.
        .max_lifetime(None)
        .idle_timeout(None)
        // By default, `Pool::acquire` on an in-memory database is not cancel-safe because it can
        // drop connections which then wipes out the database. By disabling this test we make sure
        // that when a connection is removed from the idle queue it's immediately returned to the
        // caller which makes it cancel-safe. Note also that this test is useful for client-server
        // dbs but not so much for embedded ones anyway.
        .test_before_acquire(false)
        .connect_with(connect_options)
        .await
        .map(Pool::new)
        .map_err(Error::Open)
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
    #[error("invalid storage format")]
    InvalidFormat,
    #[error("invalid storage version")]
    InvalidVersion,
    #[error("failed to execute database query")]
    Query(#[from] sqlx::Error),
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

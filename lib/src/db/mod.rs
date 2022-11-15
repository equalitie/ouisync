mod migrations;

use crate::debug;
use sqlx::{
    sqlite::{
        Sqlite, SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePoolOptions,
        SqliteTransactionBehavior,
    },
    Acquire, Row, SqlitePool,
};
use std::{
    io,
    ops::{Deref, DerefMut},
    path::Path,
    sync::Arc,
    time::Duration,
};
#[cfg(test)]
use tempfile::TempDir;
use thiserror::Error;
use tokio::{
    fs,
    sync::{Mutex as AsyncMutex, OwnedMutexGuard as AsyncOwnedMutexGuard},
};

/// Database connection pool.
#[derive(Clone)]
pub struct Pool {
    inner: SqlitePool,
    shared_tx: Arc<AsyncMutex<Option<Transaction<'static>>>>,
}

impl Pool {
    fn new(inner: SqlitePool) -> Self {
        Self {
            inner,
            shared_tx: Arc::new(AsyncMutex::new(None)),
        }
    }

    /// Acquire a database connection.
    pub async fn acquire(&self) -> Result<PoolConnection, sqlx::Error> {
        self.inner.acquire().await.map(PoolConnection)
    }

    /// Begin a regular ("unique") transaction. At most one task can hold a transaction at any time.
    /// Any other tasks are blocked on calling `begin` until the task that currently holds it is
    /// done with it (commits it or rolls it back). Performing read-only operations concurrently
    /// while a transaction is in use is still allowed. Those operations will not see the writes
    /// performed via the transaction until that transaction is committed however.
    ///
    /// If an idle `SharedTransaction` exists in the pool when `begin` is called, it is
    /// automatically committed before the regular transaction is created.
    pub async fn begin(&self) -> Result<Transaction<'static>, sqlx::Error> {
        let mut shared_tx = self.shared_tx.lock().await;

        if let Some(tx) = shared_tx.take() {
            tx.commit().await?;
        }

        Transaction::begin(&self.inner).await
    }

    /// Begin a shared transaction. Unlike regular transaction, a shared transaction is not
    /// automatically rolled back when dropped. Instead it's returned to the pool where it can be
    /// reused by calling `begin_shared` again. An idle shared transaction is auto committed when a
    /// regular transaction is created with `begin`. Shared transaction can also be manually
    /// committed or rolled back by calling `commit` or `rollback` on it respectively.
    ///
    /// Use shared transactions to group multiple writes that don't logically need to be in the
    /// same transaction to improve performance by reducing the number of commits.
    pub async fn begin_shared(&self) -> Result<SharedTransaction, sqlx::Error> {
        let mut shared_tx = self.shared_tx.clone().lock_owned().await;

        if shared_tx.is_none() {
            *shared_tx = Some(Transaction::begin(&self.inner).await?);
        }

        Ok(SharedTransaction(shared_tx))
    }

    pub(crate) async fn close(&self) {
        self.inner.close().await
    }
}

/// Database connection
// TODO: create a newtype for this which hides `begin` (use the ref-cast crate)
pub type Connection = SqliteConnection;

/// Database connection from pool
pub struct PoolConnection(sqlx::pool::PoolConnection<Sqlite>);

impl Deref for PoolConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl DerefMut for PoolConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}

const BUSY_WARNING_TIMEOUT: Duration = Duration::from_secs(5);

/// Database transaction
#[derive(Debug)]
pub struct Transaction<'a>(sqlx::Transaction<'a, Sqlite>);

impl<'a> Transaction<'a> {
    async fn begin<A>(db: A) -> Result<Transaction<'a>, sqlx::Error>
    where
        A: Acquire<'a, Database = Sqlite>,
    {
        // Use immediate transactions by default for simplicity. This is to avoid the following
        // problem that could occur with deferred transactions:
        //
        // - Connection A begins a read transaction
        // - Connection B begins a write transaction
        // - Connection A trying to upgrade to a write transaction, but the previous reads it did
        //   were invalidated by the writes by connection B which means connection A now returns
        //   `SQLITE_BUSY`. The only way to resolve that is to retry the whole transaction.
        //
        // To keep things simpler, if we know we are going to write at some point, we start the
        // transaction as a write transaction (this is what immediate does) which means the
        // `SQLITE_BUSY` can now happen only in `begin` and in most cases would be automatically
        // retried by sqlite itself. In case we still get `SQLITE_BUSY` from `begin`, all we need
        // to do is to retry the `begin` call itself which is simpler than retrying the whole
        // transaction.
        debug::warn_slow(
            BUSY_WARNING_TIMEOUT,
            "database is busy",
            db.begin_with(SqliteTransactionBehavior::Immediate.into()),
        )
        .await
        .map(Transaction)
    }

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
        self.0.deref()
    }
}

impl DerefMut for Transaction<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.deref_mut()
    }
}

/// Shared transaction
///
/// See [Pool::begin_shared] for more details.

// NOTE: The `Option` is never `None` except after `commit` or `rollback` but those methods take
// `self` by value so the `None` is never observable. So it's always OK to call `unwrap` on it.
pub struct SharedTransaction(AsyncOwnedMutexGuard<Option<Transaction<'static>>>);

impl SharedTransaction {
    pub async fn commit(mut self) -> Result<(), sqlx::Error> {
        // `unwrap` is ok, see the NONE above.
        self.0.take().unwrap().commit().await
    }

    pub async fn rollback(mut self) -> Result<(), sqlx::Error> {
        // `unwrap` is ok, see the NONE above.
        self.0.take().unwrap().rollback().await
    }
}

impl Deref for SharedTransaction {
    type Target = Transaction<'static>;

    fn deref(&self) -> &Self::Target {
        // `unwrap` is ok, see the NONE above.
        self.0.as_ref().unwrap()
    }
}

impl DerefMut for SharedTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // `unwrap` is ok, see the NONE above.
        self.0.as_mut().unwrap()
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
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);

    let pool = create_pool(connect_options).await?;

    migrations::run(&pool).await?;

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

    migrations::run(&pool).await?;

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
    let connect_options = connect_options
        .pragma("recursive_triggers", "ON")
        // We assume that every transaction completes in a reasonable time (i.e., there are no
        // deadlocks) so ideally we'd want to set infinite timeout. There is no easy way* to do
        // that so setting this excessively large timeout is the next best thing.
        //
        // *) Using a custom [busy handler](https://www.sqlite.org/c3ref/busy_handler.html) would
        //    be one such way.
        .busy_timeout(Duration::from_secs(24 * 60 * 60));

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

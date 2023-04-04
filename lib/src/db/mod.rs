#[macro_use]
mod macros;

mod connection;
mod id;
mod migrations;

pub use id::DatabaseId;

use crate::{deadlock::asynch::ExpectShortLifetime, repository_stats::RepositoryStats};
use ref_cast::RefCast;
use sqlx::{
    sqlite::{
        Sqlite, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    },
    Row, SqlitePool,
};
use std::{
    future::Future,
    io,
    ops::{Deref, DerefMut},
    panic::Location,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(test)]
use tempfile::TempDir;
use thiserror::Error;
use tokio::{
    fs,
    sync::{Mutex as AsyncMutex, OwnedMutexGuard as AsyncOwnedMutexGuard},
};
use tracing::Span;

const WARN_AFTER_TRANSACTION_LIFETIME: Duration = Duration::from_secs(3);

pub(crate) use self::connection::Connection;

/// Database connection pool.
#[derive(Clone)]
pub(crate) struct Pool {
    // Pool with multiple read-only connections
    reads: SqlitePool,
    // Pool with a single writable connection.
    write: SqlitePool,
    shared_tx: Arc<AsyncMutex<Option<WriteTransaction>>>,
    stats: Arc<RepositoryStats>,
}

impl Pool {
    async fn create(connect_options: SqliteConnectOptions) -> Result<Self, sqlx::Error> {
        let common_options = connect_options
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .pragma("recursive_triggers", "ON");

        let write_options = common_options.clone();
        let write = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(1)
            .test_before_acquire(false)
            .connect_with(write_options)
            .await?;

        let read_options = common_options.read_only(true);
        let reads = SqlitePoolOptions::new()
            .max_connections(8)
            .test_before_acquire(false)
            .connect_with(read_options)
            .await?;

        Ok(Self {
            reads,
            write,
            shared_tx: Arc::new(AsyncMutex::new(None)),
            stats: Arc::new(RepositoryStats::new(Span::current())),
        })
    }

    /// Acquire a read-only database connection.
    #[track_caller]
    pub fn acquire<'a>(&'a self) -> impl Future<Output = Result<PoolConnection, sqlx::Error>> + 'a {
        let file_and_line = Location::caller();
        self.acquire_from(file_and_line)
    }

    async fn acquire_from(
        &self,
        file_and_line: &'static Location<'static>,
    ) -> Result<PoolConnection, sqlx::Error> {
        let start = Instant::now();
        let track_lifetime =
            ExpectShortLifetime::new_at(WARN_AFTER_TRANSACTION_LIFETIME, file_and_line);
        let tx = self
            .reads
            .acquire()
            .await
            .map(|con| PoolConnection(con, track_lifetime))?;
        self.stats
            .write()
            .note_db_acquire_duration(Instant::now() - start);
        Ok(tx)
    }

    /// Begin a read-only transaction. See [`ReadTransaction`] for more details.
    #[track_caller]
    pub fn begin_read<'a>(
        &'a self,
    ) -> impl Future<Output = Result<ReadTransaction, sqlx::Error>> + 'a {
        let file_and_line = Location::caller();
        self.begin_read_from(file_and_line)
    }

    async fn begin_read_from(
        &self,
        file_and_line: &'static Location<'static>,
    ) -> Result<ReadTransaction, sqlx::Error> {
        let start = Instant::now();
        let tx = self.reads.begin().await?;
        self.stats
            .write()
            .note_db_read_begin_duration(Instant::now() - start);
        let track_lifetime =
            ExpectShortLifetime::new_at(WARN_AFTER_TRANSACTION_LIFETIME, file_and_line);
        Ok(ReadTransaction(tx, track_lifetime))
    }

    /// Begin a regular ("unique") write transaction. At most one task can hold a write transaction
    /// at any time. Any other tasks are blocked on calling `begin_write` until the task that
    /// currently holds it is done with it (commits it or rolls it back). Performing read-only
    /// operations concurrently while a write transaction is in use is still allowed. Those
    /// operations will not see the writes performed via the write transaction until that
    /// transaction is committed however.
    ///
    /// If an idle `SharedTransaction` exists in the pool when `begin_write` is called, it is
    /// automatically committed before the regular write transaction is created.
    #[track_caller]
    pub fn begin_write<'a>(
        &'a self,
    ) -> impl Future<Output = Result<WriteTransaction, sqlx::Error>> + 'a {
        let file_and_line = Location::caller();
        self.begin_write_from(file_and_line)
    }

    async fn begin_write_from(
        &self,
        file_and_line: &'static Location<'static>,
    ) -> Result<WriteTransaction, sqlx::Error> {
        let start = Instant::now();

        let mut shared_tx = self.shared_tx.lock().await;

        if let Some(tx) = shared_tx.take() {
            tx.commit().await?;
        }

        let tx = self.write.begin().await?;

        self.stats
            .write()
            .note_db_write_begin_duration(Instant::now() - start);

        let track_lifetime =
            ExpectShortLifetime::new_at(WARN_AFTER_TRANSACTION_LIFETIME, file_and_line);

        Ok(WriteTransaction(ReadTransaction(tx, track_lifetime)))
    }

    /// Begin a shared write transaction. Unlike regular write transaction, a shared write
    /// transaction is not automatically rolled back when dropped. Instead it's returned to the
    /// pool where it can be reused by calling `begin_shared_write` again. An idle shared write
    /// transaction is auto committed when a regular write transaction is created with
    /// `begin_write`. Shared write transaction can also be manually committed or rolled back by
    /// calling `commit` or `rollback` on it respectively.
    ///
    /// Use shared write transactions to group multiple writes that don't logically need to be in
    /// the same transaction to improve performance by reducing the number of commits.
    pub async fn begin_shared_write(&self) -> Result<SharedWriteTransaction, sqlx::Error> {
        let mut shared_tx = self.shared_tx.clone().lock_owned().await;

        if shared_tx.is_none() {
            let tx = self.write.begin().await?;
            let track_lifetime = ExpectShortLifetime::new(WARN_AFTER_TRANSACTION_LIFETIME);
            *shared_tx = Some(WriteTransaction(ReadTransaction(tx, track_lifetime)));
        }

        Ok(SharedWriteTransaction(shared_tx))
    }

    pub(crate) async fn close(&self) -> Result<(), sqlx::Error> {
        if let Some(tx) = self.shared_tx.lock().await.take() {
            tx.commit().await?
        }

        self.write.close().await;
        self.reads.close().await;

        Ok(())
    }

    pub(crate) fn stats(&self) -> &Arc<RepositoryStats> {
        &self.stats
    }
}

/// Database connection from pool
pub(crate) struct PoolConnection(sqlx::pool::PoolConnection<Sqlite>, ExpectShortLifetime);

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

/// Transaction that allows only reading.
///
/// This is useful if one wants to make sure the observed database content doesn't change for the
/// duration of the transaction even in the presence of concurrent writes. In other words - a read
/// transaction represents an immutable snapshot of the database at the point the transaction was
/// created. A read transaction doesn't need to be committed or rolled back - it's implicitly ended
/// when the `ReadTransaction` instance drops.
#[derive(Debug)]
pub(crate) struct ReadTransaction(sqlx::Transaction<'static, Sqlite>, ExpectShortLifetime);

impl Deref for ReadTransaction {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        Connection::ref_cast(self.0.deref())
    }
}

impl DerefMut for ReadTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Connection::ref_cast_mut(self.0.deref_mut())
    }
}

impl_executor_by_deref!(ReadTransaction);

/// Transaction that allows both reading and writing.
#[derive(Debug)]
pub(crate) struct WriteTransaction(ReadTransaction);

impl WriteTransaction {
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.0 .0.commit().await
    }
}

impl Deref for WriteTransaction {
    type Target = ReadTransaction;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for WriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl_executor_by_deref!(WriteTransaction);

/// Shared write transaction
///
/// See [Pool::begin_shared_write] for more details.

// NOTE: The `Option` is never `None` except after `commit` or `rollback` but those methods take
// `self` by value so the `None` is never observable. So it's always OK to call `unwrap` on it.
pub(crate) struct SharedWriteTransaction(AsyncOwnedMutexGuard<Option<WriteTransaction>>);

impl SharedWriteTransaction {
    pub async fn commit(mut self) -> Result<(), sqlx::Error> {
        // `unwrap` is ok, see the NOTE above.
        self.0.take().unwrap().commit().await
    }
}

impl Deref for SharedWriteTransaction {
    type Target = WriteTransaction;

    fn deref(&self) -> &Self::Target {
        // `unwrap` is ok, see the NOTE above.
        self.0.as_ref().unwrap()
    }
}

impl DerefMut for SharedWriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // `unwrap` is ok, see the NOTE above.
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
        .create_if_missing(true);

    let pool = Pool::create(connect_options).await.map_err(Error::Open)?;

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
    let pool = Pool::create(connect_options).await.map_err(Error::Open)?;

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

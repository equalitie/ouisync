use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteTransactionManager},
    Connection, SqliteConnection, TransactionManager,
};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Single db connection protected by a mutex.
///
/// NOTE: This is conceptually almost the same as `Pool` with a single connection. One important
/// difference is how `commit` behaves: It returns `CommittedMutexTransaction` which allows to
/// delay releasing the connection (unlocking the mutex) even after the transaction itself has been
/// committed.
#[derive(Clone)]
pub(super) struct ConnectionMutex(Arc<Mutex<Option<SqliteConnection>>>);

impl ConnectionMutex {
    pub async fn connect(options: SqliteConnectOptions) -> sqlx::Result<Self> {
        let conn = SqliteConnection::connect_with(&options).await?;
        Ok(Self(Arc::new(Mutex::new(Some(conn)))))
    }

    /// Begins a transaction.
    pub async fn begin(&self) -> sqlx::Result<MutexTransaction> {
        let conn = self.0.clone().lock_owned().await;
        MutexTransaction::begin(conn).await
    }

    /// Waits for the connection to be released (if checked out) and then closes it. Any subsequent
    /// attempts to check the connection out return an error.
    pub async fn close(&self) {
        let Some(conn) = self.0.lock().await.take() else {
            return;
        };

        if let Err(error) = conn.close().await {
            tracing::error!(?error, "Failed to close connection");
        }
    }
}

/// Db transaction obtained from the connection in `ConnectionMutex`.
pub(super) struct MutexTransaction {
    conn: OwnedMutexGuard<Option<SqliteConnection>>,
    closed: bool,
}

impl MutexTransaction {
    async fn begin(mut conn: OwnedMutexGuard<Option<SqliteConnection>>) -> sqlx::Result<Self> {
        SqliteTransactionManager::begin(conn.as_mut().ok_or(sqlx::Error::PoolClosed)?).await?;

        Ok(Self {
            conn,
            closed: false,
        })
    }

    /// Commits the transaction. The returned `CommittedMutexTransaction` keeps the mutex locked
    /// until it's dropped. This allows to delay the mutex unlock in order to perform operations
    /// that should be atomic with the transaction itself.
    pub async fn commit(mut self) -> sqlx::Result<CommittedMutexTransaction> {
        SqliteTransactionManager::commit(&mut self).await?;
        self.closed = true;

        Ok(CommittedMutexTransaction(self))
    }
}

impl Deref for MutexTransaction {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        // unwrap is OK because we covered the `None` case when constructing this
        // `MutexTransaction`.
        self.conn.as_ref().unwrap()
    }
}

impl DerefMut for MutexTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // unwrap is OK because we covered the `None` case when constructing this
        // `MutexTransaction`.
        self.conn.as_mut().unwrap()
    }
}

impl Drop for MutexTransaction {
    fn drop(&mut self) {
        if self.closed {
            return;
        }

        SqliteTransactionManager::start_rollback(self);
    }
}

pub(super) struct CommittedMutexTransaction(#[allow(dead_code)] MutexTransaction);

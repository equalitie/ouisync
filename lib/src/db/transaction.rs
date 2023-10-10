use super::mutex::MutexTransaction;
use sqlx::{Sqlite, SqliteConnection};
use std::ops::{Deref, DerefMut};

pub(super) enum TransactionWrapper {
    /// Transaction obtained from `sqlx::Pool`
    Pool(sqlx::Transaction<'static, Sqlite>),
    /// Transaction obtained from `ConnectionMutex`
    Mutex(MutexTransaction),
}

impl Deref for TransactionWrapper {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Pool(conn) => conn,
            Self::Mutex(conn) => conn,
        }
    }
}

impl DerefMut for TransactionWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Pool(conn) => conn,
            Self::Mutex(conn) => conn,
        }
    }
}

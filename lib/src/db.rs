use crate::error::{Error, Result};
use sqlx::{
    sqlite::{Sqlite, SqliteConnectOptions, SqliteConnection, SqlitePoolOptions},
    SqlitePool,
};
use std::{
    borrow::Cow,
    convert::Infallible,
    path::{Path, PathBuf},
    str::FromStr,
};
use tokio::fs;

/// Database connection pool.
pub(crate) type Pool = SqlitePool;

/// Database connection.
pub type Connection = SqliteConnection;

/// Database transaction
pub type Transaction<'a> = sqlx::Transaction<'a, Sqlite>;

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

/// Opens a connection to the specified database. Fails if the db doesn't exist.
pub(crate) async fn open(store: &Store) -> Result<Pool> {
    match store {
        Store::Permanent(path) => open_permanent(path, false).await,
        Store::Temporary => open_temporary().await,
    }
}

/// Opens a connection to the specified database. Creates the database if it doesn't already exist.
pub(crate) async fn open_or_create(store: &Store) -> Result<Pool> {
    match store {
        Store::Permanent(path) => open_permanent(path, true).await,
        Store::Temporary => open_temporary().await,
    }
}

async fn open_permanent(path: &Path, create_if_missing: bool) -> Result<Pool> {
    if create_if_missing {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .await
                .map_err(Error::CreateDbDirectory)?;
        }
    }

    // HACK: using only one connection to work around `SQLITE_BUSY` errors.
    //
    // TODO: After some experimentation, it seems that using `SqliteSynchornous::Normal` might fix
    // those errors but it needs more testing. But even if it works, we should try to avoid making
    // the test and the production code diverge too much. This means that in order to use multiple
    // connections we would either have to stop using memory databases or we would have to enable
    // shared cache also for file databases. Both approaches have their drawbacks.
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(create_if_missing),
        )
        .await
        .map_err(Error::ConnectToDb)
}

async fn open_temporary() -> Result<Pool> {
    // HACK: using only one connection to avoid having to use shared cache (which is
    // necessary when using multiple connections to a memory database, but it's extremely
    // prone to deadlocks)
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::from_str(MEMORY)
                .unwrap()
                .shared_cache(false),
        )
        .await
        .map_err(Error::ConnectToDb)
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

use crate::error::{Error, Result};
use sqlx::{
    pool::PoolOptions,
    sqlite::{Sqlite, SqliteConnectOptions, SqliteConnection},
    SqlitePool,
};
use std::{convert::Infallible, path::PathBuf, str::FromStr};
use tokio::fs;

/// Database connection pool.
pub type Pool = SqlitePool;

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

impl From<PathBuf> for Store {
    fn from(path: PathBuf) -> Self {
        if path.to_str() == Some(MEMORY) {
            Self::Temporary
        } else {
            Self::Permanent(path)
        }
    }
}

impl From<String> for Store {
    fn from(string: String) -> Self {
        Self::from(PathBuf::from(string))
    }
}

impl FromStr for Store {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(s.to_owned()))
    }
}

/// Opens a connection to the specified database. Fails if the db doesn't exist.
pub(crate) async fn open(store: &Store) -> Result<Pool> {
    let options = match store {
        Store::Permanent(path) => SqliteConnectOptions::new().filename(path),
        Store::Temporary => SqliteConnectOptions::from_str(MEMORY).expect("invalid db uri"),
    };

    create_pool(options).await
}

/// Opens a connection to the specified database. Creates the database if it doesn't already exist.
pub(crate) async fn open_or_create(store: &Store) -> Result<Pool> {
    let options = match store {
        Store::Permanent(path) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)
                    .await
                    .map_err(Error::CreateDbDirectory)?;
            }

            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
        }
        Store::Temporary => SqliteConnectOptions::from_str(MEMORY).expect("invalid db uri"),
    };

    create_pool(options).await
}

async fn create_pool(options: SqliteConnectOptions) -> Result<Pool> {
    PoolOptions::new()
        .connect_with(options)
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

use crate::{
    block,
    error::{Error, Result},
    index, path, this_replica,
};
use sqlx::{
    encode::IsNull,
    error::BoxDynError,
    pool::PoolOptions,
    sqlite::{Sqlite, SqliteArgumentValue, SqliteConnectOptions, SqliteTypeInfo, SqliteValueRef},
    Decode, Encode, SqlitePool, Type,
};
use std::{convert::Infallible, io, path::PathBuf, str::FromStr};
use tokio::fs;

/// Database connection pool.
pub type Pool = SqlitePool;

/// Database transaction
pub type Transaction<'a> = sqlx::Transaction<'a, Sqlite>;

/// Database connection
pub type Connection = sqlx::pool::PoolConnection<Sqlite>;

/// This trait allows to write functions that work with any of `Pool`, `Connection` or
/// `Transaction`. It's an alias for `sqlx::Executor<Database = Sqlite>` for convenience.
pub trait Executor<'a>: sqlx::Executor<'a, Database = Sqlite> {}
impl<'a, T> Executor<'a> for T where T: sqlx::Executor<'a, Database = Sqlite> {}

// URI of a memory-only db.
const MEMORY: &str = ":memory:";

/// Database store.
#[derive(Debug)]
pub enum Store {
    /// Database stored on the filesystem.
    File(PathBuf),
    /// Temporary database stored in memory.
    Memory,
}

impl FromStr for Store {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == MEMORY {
            Ok(Self::Memory)
        } else {
            Ok(Self::File(s.into()))
        }
    }
}

impl Type<Sqlite> for Store {
    fn type_info() -> SqliteTypeInfo {
        str::type_info()
    }
}

impl<'r> Decode<'r, Sqlite> for Store {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <&str>::decode(value)?;
        Ok(s.parse()?)
    }
}

impl<'q> Encode<'q, Sqlite> for &'q Store {
    fn encode_by_ref(&self, args: &mut Vec<SqliteArgumentValue<'q>>) -> IsNull {
        match self {
            Store::File(path) => {
                if let Some(s) = path.to_str() {
                    s.encode_by_ref(args)
                } else {
                    IsNull::Yes
                }
            }
            Store::Memory => MEMORY.encode_by_ref(args),
        }
    }
}

/// Creates the database unless it already exsits and establish a connection to it.
pub async fn init(store: Store) -> Result<Pool> {
    let options = match store {
        Store::File(path) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)
                    .await
                    .map_err(Error::CreateDbDirectory)?;
            }

            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
        }
        Store::Memory => SqliteConnectOptions::from_str(MEMORY).expect("invalid db uri"),
    };

    let pool = PoolOptions::new()
        // HACK: Using only one connection turns the pool effectively into a mutex over a single
        // connection. This is a heavy-handed fix that prevents the "table is locked" errors that
        // sometimes happen when multiple tasks try to access the same table and at least one of
        // them mutably. The downside is that this means only one task can access the database at
        // any given time which might affect performance.
        // TODO: find a more fine-grained way to solve this issue.
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(Error::ConnectToDb)?;

    create_schema(&pool).await?;

    Ok(pool)
}

/// Delete the database identified by the given `store`. It's unspecified what happens when there
/// are still open connection to the database (it might be OS-dependent) so it's best to make sure
/// they are all closed before this function is called.
pub async fn delete(store: Store) -> Result<()> {
    match store {
        Store::File(path) => {
            fs::remove_file(&path).await.map_err(Error::DeleteDb)?;

            // Also remove the write-ahead log and shared memory files if they exist.
            for suffix in &["-wal", "-shm"] {
                match fs::remove_file(path::os::append(&path, suffix)).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(Error::DeleteDb(error)),
                }
            }
        }
        Store::Memory => {
            // memory db is automatically dropped when the last connection is closed.
        }
    }

    Ok(())
}

// Create the database schema
pub async fn create_schema(pool: &Pool) -> Result<()> {
    block::init(pool).await?;
    index::init(pool).await?;
    this_replica::init(pool).await?;
    Ok(())
}

// Explicit cast from `i64` to `u64` to work around the lack of native `u64` support in the sqlx
// crate.
pub const fn decode_u64(i: i64) -> u64 {
    i as u64
}

// Explicit cast from `u64` to `i64` to work around the lack of native `u64` support in the sqlx
// crate.
pub const fn encode_u64(u: u64) -> i64 {
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

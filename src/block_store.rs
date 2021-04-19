use crate::block::{BlockId, BLOCK_SIZE};
use sqlx::{Row, SqlitePool};
use thiserror::Error;

pub struct BlockStore {
    pool: SqlitePool,
}

impl BlockStore {
    /// Opens block store with the given database pool.
    pub async fn open(pool: SqlitePool) -> Result<Self, Error> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS blocks (
                 name     BLOB NOT NULL,
                 version  BLOB NOT NULL,
                 auth_tag BLOB NOT NULL,
                 content  BLOB NOT NULL,
                 PRIMARY KEY (name, version)
             ) WITHOUT ROWID",
        )
        .execute(&pool)
        .await
        .map_err(Error::CreateSchema)?;

        Ok(Self { pool })
    }

    /// Reads a block from the store into a buffer.
    ///
    /// # Panics
    ///
    /// Panics if `buffer` length is less than [`BLOCK_SIZE`].
    pub async fn read(&self, id: &BlockId, buffer: &mut [u8]) -> Result<(), Error> {
        assert!(
            buffer.len() >= BLOCK_SIZE,
            "insufficient buffer length for block read"
        );

        let row =
            sqlx::query("SELECT auth_tag, content FROM blocks WHERE name = ? AND version = ?")
                .bind(id.name.as_ref())
                .bind(id.version.as_ref())
                .fetch_optional(&self.pool)
                .await;
        let row = match row {
            Ok(Some(row)) => row,
            Ok(None) => return Err(Error::BlockNotFound(*id)),
            Err(error) => return Err(Error::Query(error)),
        };

        let content: &[u8] = row.try_get(1).map_err(Error::Query)?;
        if content.len() != BLOCK_SIZE {
            return Err(Error::WrongBlockLength(content.len()));
        }

        buffer.copy_from_slice(content);

        // TODO: return auth tag

        Ok(())
    }

    /// Writes a block into the store.
    ///
    /// # Panics
    ///
    /// Panics if buffer length is not equal to [`BLOCK_SIZE`].
    ///
    pub async fn write(&self, id: &BlockId, buffer: &[u8]) -> Result<(), Error> {
        assert_eq!(
            buffer.len(),
            BLOCK_SIZE,
            "incorrect buffer length for block write"
        );

        // TODO:
        let auth_tag = [0u8; 16];

        let result = sqlx::query(
            "INSERT INTO blocks (name, version, auth_tag, content) VALUES (?, ?, ?, ?)",
        )
        .bind(id.name.as_ref())
        .bind(id.version.as_ref())
        .bind(&auth_tag[..])
        .bind(buffer)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(error @ sqlx::Error::Database(_)) => {
                // Check whether the error is due to duplicate record or some other reason.
                // TODO: is there a way to make the update and this check atomic?
                let row = sqlx::query("SELECT 1 FROM blocks WHERE name = ? AND version = ?")
                    .bind(id.name.as_ref())
                    .bind(id.version.as_ref())
                    .fetch_optional(&self.pool)
                    .await;

                if row.map(|row| row.is_some()).unwrap_or(false) {
                    Err(Error::BlockExists(*id))
                } else {
                    Err(Error::Query(error))
                }
            }
            Err(error) => Err(Error::Query(error)),
        }
    }
}

/// BlockStore error
#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to create database schema")]
    CreateSchema(#[source] sqlx::Error),
    #[error("failed to execute database query")]
    Query(#[source] sqlx::Error),
    #[error("block not found: {0}")]
    BlockNotFound(BlockId),
    #[error("block already exists: {0}")]
    BlockExists(BlockId),
    #[error("block has wrong length (expected: {}, actual: {0})", BLOCK_SIZE)]
    WrongBlockLength(usize),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockName, BlockVersion};
    use rand::Rng;

    #[tokio::test(flavor = "multi_thread")]
    async fn write_and_read() {
        let pool = make_pool().await;
        let store = BlockStore::open(pool).await.unwrap();

        let id = random_block_id();
        let content = random_block_content();

        store.write(&id, &content).await.unwrap();

        let mut buffer = vec![0; BLOCK_SIZE];
        store.read(&id, &mut buffer).await.unwrap();

        assert_eq!(buffer, content);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_read_missing_block() {
        let pool = make_pool().await;
        let store = BlockStore::open(pool).await.unwrap();
        let id = random_block_id();

        let mut buffer = vec![0; BLOCK_SIZE];

        match store.read(&id, &mut buffer).await {
            Err(Error::BlockNotFound(missing_id)) => assert_eq!(missing_id, id),
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_write_existing_block() {
        let pool = make_pool().await;
        let store = BlockStore::open(pool).await.unwrap();

        let id = random_block_id();
        let content0 = random_block_content();

        store.write(&id, &content0).await.unwrap();

        let content1 = random_block_content();

        match store.write(&id, &content1).await {
            Err(Error::BlockExists(existing_id)) => assert_eq!(existing_id, id),
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    async fn make_pool() -> SqlitePool {
        SqlitePool::connect(":memory:").await.unwrap()
    }

    fn random_block_id() -> BlockId {
        BlockId::new(BlockName::random(), BlockVersion::random())
    }

    fn random_block_content() -> Vec<u8> {
        let mut content = vec![0; BLOCK_SIZE];
        rand::thread_rng().fill(&mut content[..]);
        content
    }
}

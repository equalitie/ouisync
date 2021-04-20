use super::{BlockId, BLOCK_SIZE};
use crate::{db, error::Error};
use sqlx::Row;

pub struct BlockStore {
    pool: db::Pool,
}

impl BlockStore {
    /// Opens block store using the given database pool.
    pub async fn open(pool: db::Pool) -> Result<Self, Error> {
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
        .map_err(Error::CreateDbSchema)?;

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
            Err(error) => return Err(Error::QueryDb(error)),
        };

        let content: &[u8] = row.try_get(1).map_err(Error::QueryDb)?;
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
                    Err(Error::QueryDb(error))
                }
            }
            Err(error) => Err(Error::QueryDb(error)),
        }
    }
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

    async fn make_pool() -> db::Pool {
        db::Pool::connect(":memory:").await.unwrap()
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

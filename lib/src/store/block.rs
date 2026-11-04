use super::error::Error;
use crate::{
    db::{self, MAX_BIND_PARAMS},
    protocol::{BLOCK_SIZE, Block, BlockContent, BlockId, BlockNonce},
};
use sqlx::{QueryBuilder, Row, Sqlite};

/// Reads a block from the store into a buffer.
///
/// # Panics
///
/// Panics if `buffer` length is less than [`BLOCK_SIZE`].
pub(super) async fn read(
    conn: &mut db::Connection,
    id: &BlockId,
    content: &mut BlockContent,
) -> Result<BlockNonce, Error> {
    assert!(
        content.len() >= BLOCK_SIZE,
        "insufficient buffer length for block read"
    );

    let row = sqlx::query("SELECT nonce, content FROM blocks WHERE id = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(Error::BlockNotFound)?;

    let nonce: &[u8] = row.get(0);
    let nonce = BlockNonce::try_from(nonce).map_err(|_| Error::MalformedData)?;

    let src_content: &[u8] = row.get(1);
    if src_content.len() != BLOCK_SIZE {
        tracing::error!(
            expected = BLOCK_SIZE,
            actual = src_content.len(),
            "Wrong block length"
        );
        return Err(Error::MalformedData);
    }

    content.copy_from_slice(src_content);

    Ok(nonce)
}

/// Write blocks to the store in batch.
pub(super) struct BlockWriter<'a> {
    tx: &'a mut db::WriteTransaction,
    builder: QueryBuilder<'a, Sqlite>,
    count: usize,
}

impl<'a> BlockWriter<'a> {
    pub fn new(tx: &'a mut db::WriteTransaction) -> Self {
        Self {
            tx,
            builder: QueryBuilder::new("INSERT INTO blocks (id, nonce, content) VALUES "),
            count: 0,
        }
    }

    /// Adds block to the batch
    pub async fn add(&mut self, block: &'a Block) -> Result<&mut Self, Error> {
        assert_eq!(
            block.content.len(),
            BLOCK_SIZE,
            "incorrect buffer length for block write"
        );

        if self.count >= MAX_BIND_PARAMS / 3 {
            self.execute().await?;
        }

        if self.count > 0 {
            self.builder.push(", ");
        }

        self.builder.push("(");
        self.builder.push_bind(&block.id);
        self.builder.push(", ");
        self.builder.push_bind(&block.nonce[..]);
        self.builder.push(", ");
        self.builder.push_bind(&block.content[..]);
        self.builder.push(")");

        self.count += 1;

        Ok(self)
    }

    /// Write all batched blocks to the store.
    pub async fn finish(&mut self) -> Result<(), Error> {
        self.execute().await
    }

    async fn execute(&mut self) -> Result<(), Error> {
        if self.count == 0 {
            return Ok(());
        }

        self.builder.push(" ON CONFLICT (id) DO NOTHING");
        self.builder
            .build()
            .persistent(false)
            .execute(&mut *self.tx)
            .await?;
        self.builder.reset();
        self.count = 0;

        Ok(())
    }
}

pub(super) async fn remove(tx: &mut db::WriteTransaction, id: &BlockId) -> Result<(), Error> {
    sqlx::query("DELETE FROM blocks WHERE id = ?")
        .bind(id)
        .execute(tx)
        .await?;

    Ok(())
}

/// Returns the total number of blocks in the store.
pub(super) async fn count(conn: &mut db::Connection) -> Result<u64, Error> {
    Ok(db::decode_u64(
        sqlx::query("SELECT COUNT(*) FROM blocks")
            .fetch_one(conn)
            .await?
            .get(0),
    ))
}

/// Checks whether the block exists in the store.
#[cfg(test)]
pub(super) async fn exists(conn: &mut db::Connection, id: &BlockId) -> Result<bool, Error> {
    Ok(sqlx::query("SELECT 0 FROM blocks WHERE id = ?")
        .bind(id)
        .fetch_optional(conn)
        .await?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn write_and_read_block() {
        let (_base_dir, pool) = setup().await;

        let block: Block = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        BlockWriter::new(&mut tx)
            .add(&block)
            .await
            .unwrap()
            .finish()
            .await
            .unwrap();

        let mut content = BlockContent::new();
        read(&mut tx, &block.id, &mut content).await.unwrap();

        assert_eq!(&content[..], &block.content[..]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_read_missing_block() {
        let (_base_dir, pool) = setup().await;

        let mut content = BlockContent::new();
        let nonce = rand::random();
        let id = BlockId::new(&content, &nonce);

        let mut conn = pool.acquire().await.unwrap();

        match read(&mut conn, &id, &mut content).await {
            Err(Error::BlockNotFound) => (),
            Err(error) => panic!("unexpected error: {error:?}"),
            Ok(_) => panic!("unexpected success"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_write_existing_block() {
        let (_base_dir, pool) = setup().await;

        let block: Block = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        for _ in 0..2 {
            BlockWriter::new(&mut tx)
                .add(&block)
                .await
                .unwrap()
                .finish()
                .await
                .unwrap();
        }
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}

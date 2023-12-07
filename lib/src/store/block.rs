use super::{
    cache::CacheTransaction,
    error::Error,
    index::{self, UpdateSummaryReason},
    leaf_node,
};
use crate::{
    db,
    protocol::{Block, BlockContent, BlockId, BlockNonce, BLOCK_SIZE},
};
use futures_util::TryStreamExt;
use sqlx::Row;

/// Write a block received from a remote replica.
pub(super) async fn receive(
    write_tx: &mut db::WriteTransaction,
    cache_tx: &mut CacheTransaction,
    block: &Block,
) -> Result<(), Error> {
    if !leaf_node::set_present(write_tx, &block.id).await? {
        return Ok(());
    }

    let nodes: Vec<_> = leaf_node::load_parent_hashes(write_tx, &block.id)
        .try_collect()
        .await?;

    index::update_summaries(write_tx, cache_tx, nodes, UpdateSummaryReason::Other).await?;

    write(write_tx, block).await?;

    Ok(())
}

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
        .ok_or_else(|| {
            tracing::trace!(?id, "Block not found");
            Error::BlockNotFound
        })?;

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

/// Writes a block into the store.
///
/// If a block with the same id already exists, this is a no-op.
///
/// # Panics
///
/// Panics if buffer length is not equal to [`BLOCK_SIZE`].
///
pub(super) async fn write(tx: &mut db::WriteTransaction, block: &Block) -> Result<(), Error> {
    assert_eq!(
        block.content.len(),
        BLOCK_SIZE,
        "incorrect buffer length for block write"
    );

    sqlx::query(
        "INSERT INTO blocks (id, nonce, content)
         VALUES (?, ?, ?)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&block.id)
    .bind(&block.nonce[..])
    .bind(&block.content[..])
    .execute(tx)
    .await?;

    Ok(())
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

        write(&mut tx, &block).await.unwrap();

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
            Err(error) => panic!("unexpected error: {:?}", error),
            Ok(_) => panic!("unexpected success"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn try_write_existing_block() {
        let (_base_dir, pool) = setup().await;

        let block: Block = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        write(&mut tx, &block).await.unwrap();
        write(&mut tx, &block).await.unwrap();
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}

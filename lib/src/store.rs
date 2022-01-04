//! Operation that affect both the index and the block store.

use crate::{
    block::{self, BlockId},
    crypto::cipher::AuthTag,
    db,
    error::Result,
    index::{self, Index},
};
use sqlx::Acquire;

/// Write a block received from a remote replica to the block store. The block must already be
/// referenced by the index, otherwise an `BlockNotReferenced` error is returned.
pub(crate) async fn write_received_block(
    index: &Index,
    id: &BlockId,
    content: &[u8],
    auth_tag: &AuthTag,
) -> Result<()> {
    let mut cx = index.pool.acquire().await?;
    let mut tx = cx.begin().await?;

    let writer_ids = index::receive_block(&mut tx, id).await?;
    block::write(&mut tx, id, content, auth_tag).await?;
    tx.commit().await?;

    let branches = index.branches().await;

    for writer_id in &writer_ids {
        if let Some(branch) = branches.get(writer_id) {
            branch.reload_root(&mut cx).await?;
            branch.notify().await;
        }
    }

    Ok(())
}

/// Initialize database objects to support operations that affect both the index and the block
/// store.
pub(crate) async fn init(pool: &db::Pool) -> Result<()> {
    // Create trigger to delete orphaned blocks.
    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS blocks_delete_on_leaf_node_deleted
         AFTER DELETE ON snapshot_leaf_nodes
         WHEN NOT EXISTS (SELECT 0 FROM snapshot_leaf_nodes WHERE block_id = old.block_id)
         BEGIN
             DELETE FROM blocks WHERE id = old.block_id;
         END;",
    )
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        block::{self, BLOCK_SIZE},
        crypto::cipher::{AuthTag, SecretKey},
        db,
        index::{self, BranchData},
        locator::Locator,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_block() {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        block::init(&pool).await.unwrap();
        super::init(&pool).await.unwrap();

        let secret_key = SecretKey::random();
        let (notify_tx, _) = async_broadcast::broadcast(1);

        let branch0 = BranchData::new(&pool, rand::random(), notify_tx.clone())
            .await
            .unwrap();
        let branch1 = BranchData::new(&pool, rand::random(), notify_tx)
            .await
            .unwrap();

        let block_id = rand::random();
        let buffer = vec![0; BLOCK_SIZE];

        let mut tx = pool.begin().await.unwrap();

        block::write(&mut tx, &block_id, &buffer, &AuthTag::default())
            .await
            .unwrap();

        let locator0 = Locator::head(rand::random());
        let locator0 = locator0.encode(&secret_key);
        branch0.insert(&mut tx, &block_id, &locator0).await.unwrap();

        let locator1 = Locator::head(rand::random());
        let locator1 = locator1.encode(&secret_key);
        branch1.insert(&mut tx, &block_id, &locator1).await.unwrap();

        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch0.remove(&mut tx, &locator0).await.unwrap();
        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch1.remove(&mut tx, &locator1).await.unwrap();
        assert!(!block::exists(&mut tx, &block_id).await.unwrap(),);
    }
}

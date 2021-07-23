//! Operation that affect both the index and the block store.

use crate::{
    block::{self, BlockId},
    crypto::AuthTag,
    db,
    error::Result,
    index::{self, Index},
};

/// Removes the block if it's orphaned (not referenced by any branch), otherwise does nothing.
/// Returns whether the block was removed.
pub async fn remove_orphaned_block(tx: &mut db::Transaction<'_>, id: &BlockId) -> Result<bool> {
    let result = sqlx::query(
        "DELETE FROM blocks
         WHERE id = ? AND NOT EXISTS (SELECT 1 FROM snapshot_leaf_nodes WHERE block_id = id)",
    )
    .bind(id)
    .execute(tx)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Write a block received from a remote replica to the block store. The block must already be
/// referenced by the index, otherwise an `BlockNotReferenced` error is returned.
pub async fn write_received_block(
    index: &Index,
    id: &BlockId,
    content: &[u8],
    auth_tag: &AuthTag,
) -> Result<()> {
    let mut tx = index.pool.begin().await?;

    let replica_ids = index::receive_block(&mut tx, id).await?;
    block::write(&mut tx, id, content, auth_tag).await?;
    tx.commit().await?;

    index.notify_branches_changed(&replica_ids).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blob_id::BlobId,
        block::{self, BLOCK_SIZE},
        crypto::{AuthTag, Cryptor},
        index::{self, BranchData},
        locator::Locator,
        replica_id::ReplicaId,
    };

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_block() {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        block::init(&pool).await.unwrap();

        let cryptor = Cryptor::Null;

        let branch0 = BranchData::new(&pool, ReplicaId::random()).await.unwrap();
        let branch1 = BranchData::new(&pool, ReplicaId::random()).await.unwrap();

        let block_id = BlockId::random();
        let buffer = vec![0; BLOCK_SIZE];

        let mut tx = pool.begin().await.unwrap();

        block::write(&mut tx, &block_id, &buffer, &AuthTag::default())
            .await
            .unwrap();

        let locator0 = Locator::Head(BlobId::random());
        let locator0 = locator0.encode(&cryptor);
        branch0.insert(&mut tx, &block_id, &locator0).await.unwrap();

        let locator1 = Locator::Head(BlobId::random());
        let locator1 = locator1.encode(&cryptor);
        branch1.insert(&mut tx, &block_id, &locator1).await.unwrap();

        assert!(!remove_orphaned_block(&mut tx, &block_id).await.unwrap());
        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch0.remove(&mut tx, &locator0).await.unwrap();

        assert!(!remove_orphaned_block(&mut tx, &block_id).await.unwrap());
        assert!(block::exists(&mut tx, &block_id).await.unwrap());

        branch1.remove(&mut tx, &locator1).await.unwrap();

        assert!(remove_orphaned_block(&mut tx, &block_id).await.unwrap());
        assert!(!block::exists(&mut tx, &block_id).await.unwrap(),);
    }
}

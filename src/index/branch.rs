// This is temporary to avoid lint errors when INNER_LAYER_COUNT = 0
#![allow(clippy::reversed_empty_ranges)]
#![allow(clippy::absurd_extreme_comparisons)]
#![allow(arithmetic_overflow)]

use super::{
    node::{InnerNode, LeafNode, RootNode},
    path::Path,
    INNER_LAYER_COUNT,
};
use crate::{
    block::BlockId,
    crypto::Hash,
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
};
use std::{mem, sync::Arc};
use tokio::sync::Mutex;

type LocatorHash = Hash;

pub struct Branch {
    root_node: Arc<Mutex<RootNode>>,
    replica_id: ReplicaId,
}

impl Branch {
    pub async fn new(tx: &mut db::Transaction, replica_id: ReplicaId) -> Result<Self> {
        let root_node = RootNode::load_latest_or_create(tx, &replica_id).await?;

        Ok(Self {
            root_node: Arc::new(Mutex::new(root_node)),
            replica_id,
        })
    }

    /// Inserts a new block into the index. Returns the previous id at the same locator, if any.
    pub async fn insert(
        &self,
        tx: &mut db::Transaction,
        block_id: &BlockId,
        encoded_locator: &LocatorHash,
    ) -> Result<Option<BlockId>> {
        let mut lock = self.root_node.lock().await;
        let mut path = self.get_path(tx, &lock.hash, &encoded_locator).await?;

        // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
        // hit one in 2^sizeof(BlockVersion) chance that we randomly generated the same
        // BlockVersion twice.
        assert!(!path.has_leaf(block_id));

        let old_block_id = path.set_leaf(&block_id);
        let old_root = self.write_path(tx, &mut lock, &path).await?;
        self.remove_snapshot(&old_root, tx).await?;

        Ok(old_block_id)
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(&self, tx: &mut db::Transaction, encoded_locator: &Hash) -> Result<BlockId> {
        let root_node = self.root_node.lock().await;
        let path = self.get_path(tx, &root_node.hash, &encoded_locator).await?;

        match path.get_leaf() {
            Some(block_id) => Ok(block_id),
            None => Err(Error::EntryNotFound),
        }
    }

    /// Remove the block identified by encoded_locator from the index. Returns the id of the
    /// removed block.
    pub async fn remove(
        &self,
        tx: &mut db::Transaction,
        encoded_locator: &Hash,
    ) -> Result<BlockId> {
        let mut lock = self.root_node.lock().await;
        let mut path = self.get_path(tx, &lock.hash, encoded_locator).await?;
        let block_id = path
            .remove_leaf(encoded_locator)
            .ok_or(Error::EntryNotFound)?;
        let old_root = self.write_path(tx, &mut lock, &path).await?;
        self.remove_snapshot(&old_root, tx).await?;

        Ok(block_id)
    }

    async fn get_path(
        &self,
        tx: &mut db::Transaction,
        root_hash: &Hash,
        encoded_locator: &LocatorHash,
    ) -> Result<Path> {
        let mut path = Path::new(*root_hash, *encoded_locator);

        path.layers_found += 1;

        let mut parent = path.root_hash;

        for level in 0..INNER_LAYER_COUNT {
            path.inner[level] = InnerNode::load_children(tx, &parent).await?;

            if let Some(node) = path.inner[level].get(path.get_bucket(level)) {
                parent = node.hash
            } else {
                return Ok(path);
            };

            path.layers_found += 1;
        }

        path.leaves = LeafNode::load_children(tx, &parent).await?;

        if path.leaves.get(encoded_locator).is_some() {
            path.layers_found += 1;
        }

        Ok(path)
    }

    async fn write_path(
        &self,
        tx: &mut db::Transaction,
        root_node: &mut RootNode,
        path: &Path,
    ) -> Result<RootNode> {
        for (i, inner_layer) in path.inner.iter().enumerate() {
            if let Some(parent_hash) = path.hash_at_layer(i) {
                for (bucket, node) in inner_layer {
                    node.save(tx, &parent_hash, bucket).await?;
                }
            }
        }

        let layer = Path::total_layer_count() - 1;
        if let Some(parent_hash) = path.hash_at_layer(layer - 1) {
            for leaf in &path.leaves {
                leaf.save(tx, &parent_hash).await?;
            }
        }

        self.write_branch_root(tx, root_node, path.root_hash).await
    }

    async fn write_branch_root(
        &self,
        tx: &mut db::Transaction,
        node: &mut RootNode,
        hash: Hash,
    ) -> Result<RootNode> {
        let new_root = node.clone_with_new_hash(tx, hash).await?;
        Ok(mem::replace(node, new_root))
    }

    async fn remove_snapshot(&self, root_node: &RootNode, tx: &mut db::Transaction) -> Result<()> {
        root_node.remove_recursive(tx).await
    }
}

impl Clone for Branch {
    fn clone(&self) -> Self {
        Self {
            root_node: self.root_node.clone(),
            replica_id: self.replica_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::{Cryptor, Hashable},
        index,
        locator::Locator,
    };
    use sqlx::Row;

    #[tokio::test(flavor = "multi_thread")]
    async fn insert_and_read() {
        let pool = init_db().await;

        let mut tx = pool.begin().await.unwrap();
        let branch = Branch::new(&mut tx, ReplicaId::random()).await.unwrap();
        tx.commit().await.unwrap();

        let block_id = BlockId::random();
        let locator = random_head_locator(0);
        let encoded_locator = locator.encode(&Cryptor::Null);

        let mut tx = pool.begin().await.unwrap();

        branch
            .insert(&mut tx, &block_id, &encoded_locator)
            .await
            .unwrap();

        let r = branch.get(&mut tx, &encoded_locator).await.unwrap();

        assert_eq!(r, block_id);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rewrite_locator() {
        for _ in 0..32 {
            let pool = init_db().await;
            let mut tx = pool.begin().await.unwrap();

            let branch = Branch::new(&mut tx, ReplicaId::random()).await.unwrap();

            let b1 = BlockId::random();
            let b2 = BlockId::random();

            let locator = random_head_locator(0);
            let encoded_locator = locator.encode(&Cryptor::Null);

            branch.insert(&mut tx, &b1, &encoded_locator).await.unwrap();
            branch.insert(&mut tx, &b2, &encoded_locator).await.unwrap();

            let r = branch.get(&mut tx, &encoded_locator).await.unwrap();

            assert_eq!(r, b2);

            assert_eq!(
                INNER_LAYER_COUNT + 1,
                count_branch_forest_entries(&mut tx).await
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_locator() {
        let pool = init_db().await;
        let mut tx = pool.begin().await.unwrap();

        let branch = Branch::new(&mut tx, ReplicaId::random()).await.unwrap();

        let b = BlockId::random();
        let locator = random_head_locator(0);
        let encoded_locator = locator.encode(&Cryptor::Null);

        assert_eq!(0, count_branch_forest_entries(&mut tx).await);

        {
            branch.insert(&mut tx, &b, &encoded_locator).await.unwrap();
            let r = branch.get(&mut tx, &encoded_locator).await.unwrap();
            assert_eq!(r, b);
        }

        assert_eq!(
            INNER_LAYER_COUNT + 1,
            count_branch_forest_entries(&mut tx).await
        );

        {
            branch.remove(&mut tx, &encoded_locator).await.unwrap();

            match branch.get(&mut tx, &encoded_locator).await {
                Err(Error::EntryNotFound) => { /* OK */ }
                Err(_) => panic!("Error should have been EntryNotFound"),
                Ok(_) => panic!("Branch shouldn't have contained the block ID"),
            }
        }

        assert_eq!(0, count_branch_forest_entries(&mut tx).await);
    }

    async fn count_branch_forest_entries(tx: &mut db::Transaction) -> usize {
        sqlx::query(
            "SELECT
                 (SELECT COUNT(*) FROM snapshot_inner_nodes) +
                 (SELECT COUNT(*) FROM snapshot_leaf_nodes)",
        )
        .fetch_one(tx)
        .await
        .unwrap()
        .get::<u32, _>(0) as usize
    }

    async fn init_db() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        pool
    }

    fn random_head_locator(seq: u32) -> Locator {
        Locator::Head(rand::random::<u64>().hash(), seq)
    }
}

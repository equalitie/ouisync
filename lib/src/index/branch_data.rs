// This is temporary to avoid lint errors when INNER_LAYER_COUNT = 0
#![allow(clippy::reversed_empty_ranges)]
#![allow(clippy::absurd_extreme_comparisons)]
#![allow(arithmetic_overflow)]

use super::{
    node::{InnerNode, LeafNode, RootNode, INNER_LAYER_COUNT},
    path::Path,
};
use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, Hash},
    db,
    error::{Error, Result},
    version_vector::VersionVector,
};
use std::mem;
use tokio::sync::{watch, RwLock, RwLockReadGuard};

type LocatorHash = Hash;

pub(crate) struct BranchData {
    writer_id: PublicKey,
    root_node: RwLock<RootNode>,
    changed_tx: watch::Sender<()>,
}

impl BranchData {
    pub async fn new(pool: &db::Pool, writer_id: PublicKey) -> Result<Self> {
        let root_node = RootNode::load_latest_or_create(pool, &writer_id).await?;
        Ok(Self::with_root_node(writer_id, root_node))
    }

    pub fn with_root_node(writer_id: PublicKey, root_node: RootNode) -> Self {
        let (changed_tx, _) = watch::channel(());

        Self {
            writer_id,
            root_node: RwLock::new(root_node),
            changed_tx,
        }
    }

    /// Returns the id of the replica that owns this branch.
    pub fn id(&self) -> &PublicKey {
        &self.writer_id
    }

    /// Returns the root version vector of this branch.
    pub async fn root_version_vector(&self) -> RwLockReadGuard<'_, VersionVector> {
        RwLockReadGuard::map(self.root_node.read().await, |root| &root.versions)
    }

    /// Update the root version vector of this branch.
    pub async fn update_root_version_vector(
        &self,
        tx: db::Transaction<'_>,
        version_vector_override: Option<&VersionVector>,
    ) -> Result<()> {
        // TODO: should we emit a notification event here?

        self.root_node
            .write()
            .await
            .update_version_vector(tx, version_vector_override)
            .await
    }

    /// Inserts a new block into the index.
    pub async fn insert(
        &self,
        tx: &mut db::Transaction<'_>,
        block_id: &BlockId,
        encoded_locator: &LocatorHash,
    ) -> Result<()> {
        let mut lock = self.root_node.write().await;
        let mut path = self.get_path(tx, &lock.hash, encoded_locator).await?;

        // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
        // hit one in 2^sizeof(BlockVersion) chance that we randomly generated the same
        // BlockVersion twice.
        assert!(!path.has_leaf(block_id));

        path.set_leaf(block_id);
        self.write_path(tx, &mut lock, &path).await
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(
        &self,
        tx: &mut db::Transaction<'_>,
        encoded_locator: &Hash,
    ) -> Result<BlockId> {
        let root_node = self.root_node.read().await;
        let path = self.get_path(tx, &root_node.hash, encoded_locator).await?;

        match path.get_leaf() {
            Some(block_id) => Ok(block_id),
            None => Err(Error::EntryNotFound),
        }
    }

    /// Remove the block identified by encoded_locator from the index. Returns the id of the
    /// removed block.
    pub async fn remove(&self, tx: &mut db::Transaction<'_>, encoded_locator: &Hash) -> Result<()> {
        let mut lock = self.root_node.write().await;
        let mut path = self.get_path(tx, &lock.hash, encoded_locator).await?;
        path.remove_leaf(encoded_locator)
            .ok_or(Error::EntryNotFound)?;
        self.write_path(tx, &mut lock, &path).await
    }

    /// Subscribe to notifications of changes in this branch. A notification is emitted every time
    /// a new snapshot of this branch is created or a previously missing block is downloaded.
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.changed_tx.subscribe()
    }

    /// Trigger a notification event from this branch.
    pub(super) fn notify_changed(&self) {
        self.changed_tx.send(()).unwrap_or(())
    }

    /// Update the root node of this branch. Does nothing if the version of `new_root` is not
    /// greater than the version of the current root.
    pub async fn update_root(
        &self,
        tx: &mut db::Transaction<'_>,
        new_root: RootNode,
    ) -> Result<()> {
        let mut old_root = self.root_node.write().await;

        if new_root.versions.get(&self.writer_id) <= old_root.versions.get(&self.writer_id) {
            return Ok(());
        }

        self.replace_root(tx, &mut old_root, new_root).await
    }

    async fn get_path(
        &self,
        tx: &mut db::Transaction<'_>,
        root_hash: &Hash,
        encoded_locator: &LocatorHash,
    ) -> Result<Path> {
        let mut path = Path::new(*root_hash, *encoded_locator);

        path.layers_found += 1;

        let mut parent = path.root_hash;

        for level in 0..INNER_LAYER_COUNT {
            path.inner[level] = InnerNode::load_children(&mut *tx, &parent).await?;

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

    // TODO: make sure nodes are saved as complete.
    async fn write_path(
        &self,
        tx: &mut db::Transaction<'_>,
        old_root: &mut RootNode,
        path: &Path,
    ) -> Result<()> {
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

        let new_root = old_root.next_version(tx, path.root_hash).await?;
        self.replace_root(tx, old_root, new_root).await
    }

    async fn replace_root(
        &self,
        tx: &mut db::Transaction<'_>,
        old_root: &mut RootNode,
        new_root: RootNode,
    ) -> Result<()> {
        let old_root = mem::replace(old_root, new_root);
        old_root.remove_recursive(tx).await?;
        self.notify_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::Cryptor, index, locator::Locator};
    use sqlx::Row;

    #[tokio::test(flavor = "multi_thread")]
    async fn insert_and_read() {
        let pool = init_db().await;
        let branch = BranchData::new(&pool, rand::random()).await.unwrap();

        let block_id = rand::random();
        let locator = random_head_locator();
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
            let branch = BranchData::new(&pool, rand::random()).await.unwrap();

            let b1 = rand::random();
            let b2 = rand::random();

            let locator = random_head_locator();
            let encoded_locator = locator.encode(&Cryptor::Null);

            let mut tx = pool.begin().await.unwrap();

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
        let branch = BranchData::new(&pool, rand::random()).await.unwrap();

        let b = rand::random();
        let locator = random_head_locator();
        let encoded_locator = locator.encode(&Cryptor::Null);

        let mut tx = pool.begin().await.unwrap();

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
                Ok(_) => panic!("BranchData shouldn't have contained the block ID"),
            }
        }

        assert_eq!(0, count_branch_forest_entries(&mut tx).await);
    }

    async fn count_branch_forest_entries(tx: &mut db::Transaction<'_>) -> usize {
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

    fn random_head_locator() -> Locator {
        Locator::head(rand::random())
    }
}

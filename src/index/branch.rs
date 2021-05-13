// This is temporary to avoid lint errors when INNER_LAYER_COUNT = 0
#![allow(clippy::reversed_empty_ranges)]
#![allow(clippy::absurd_extreme_comparisons)]
#![allow(arithmetic_overflow)]

use crate::{
    block::BlockId,
    crypto::Hash,
    db,
    error::{Error, Result},
    index::node::{inner_children, leaf_children, InnerNode, LeafNode, RootNode},
    index::LocatorHash,
    index::{INNER_LAYER_COUNT, MAX_INNER_NODE_CHILD_COUNT},
    replica_id::ReplicaId,
};
use sha3::{Digest, Sha3_256};
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};

type SnapshotId = u32;

type Lock<'a> = MutexGuard<'a, RootNode>;

pub struct Branch {
    root_node: Arc<Mutex<RootNode>>,
    replica_id: ReplicaId,
}

impl Branch {
    pub async fn new(pool: db::Pool, replica_id: ReplicaId) -> Result<Self> {
        let root_node = RootNode::get_latest_or_create(pool, &replica_id).await?;

        Ok(Self {
            root_node: Arc::new(Mutex::new(root_node)),
            replica_id,
        })
    }

    pub fn clone(&self) -> Self {
        Self {
            root_node: self.root_node.clone(),
            replica_id: self.replica_id,
        }
    }

    /// Insert a new block into the index.
    pub async fn insert(
        &self,
        tx: &mut db::Transaction,
        block_id: &BlockId,
        encoded_locator: &LocatorHash,
    ) -> Result<()> {
        let mut lock = self.lock().await;
        let mut path = self.get_path(tx, &lock.root_hash, &encoded_locator).await?;

        // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
        // hit one in 2^sizeof(BlockVersion) chance that we randomly generated the same
        // BlockVersion twice.
        assert!(!path.has_leaf(block_id));

        path.insert_leaf(&block_id);
        let old_root = self.write_path(tx, &mut lock, &path).await?;
        self.remove_snapshot(&old_root, tx).await
    }

    /// Insert the root block into the index
    pub async fn insert_root(&self, tx: &mut db::Transaction, block_id: &BlockId) -> Result<()> {
        self.insert(tx, block_id, &Hash::null()).await
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(&self, tx: &mut db::Transaction, encoded_locator: &Hash) -> Result<BlockId> {
        let lock = self.lock().await;

        if lock.root_hash.is_null() {
            return Err(Error::BlockIdNotFound);
        }

        let path = self.get_path(tx, &lock.root_hash, &encoded_locator).await?;

        match path.get_leaf(encoded_locator) {
            Some(block_id) => Ok(block_id),
            None => Err(Error::BlockIdNotFound),
        }
    }

    /// Get the root block from the index.
    pub async fn get_root(&self, tx: &mut db::Transaction) -> Result<BlockId> {
        self.get(tx, &Hash::null()).await
    }

    /// Remove the block identified by encoded_locator from the index
    pub async fn remove(&self, tx: &mut db::Transaction, encoded_locator: &Hash) -> Result<()> {
        let mut lock = self.lock().await;
        let mut path = self.get_path(tx, &lock.root_hash, encoded_locator).await?;
        path.remove_leaf(encoded_locator);
        let old_root = self.write_path(tx, &mut lock, &path).await?;
        self.remove_snapshot(&old_root, tx).await
    }

    /// Remove the root block from the index
    pub async fn remove_root(&self, tx: &mut db::Transaction) -> Result<()> {
        self.remove(tx, &Hash::null()).await
    }

    async fn get_path(
        &self,
        tx: &mut db::Transaction,
        root_hash: &Hash,
        encoded_locator: &LocatorHash,
    ) -> Result<PathWithSiblings> {
        let mut path = PathWithSiblings::new(&root_hash, *encoded_locator);

        if path.root.is_null() {
            return Ok(path);
        }

        path.layers_found += 1;

        let mut parent = path.root;

        for level in 0..INNER_LAYER_COUNT {
            path.inner[level] = inner_children(&parent, tx).await?;
            parent = path.inner[level][path.get_bucket(level)].hash;

            if parent.is_null() {
                return Ok(path);
            }

            path.layers_found += 1;
        }

        path.leaves = leaf_children(&parent, tx).await?;

        let has_locator = path.leaves.iter().any(|l| l.locator == *encoded_locator);

        if has_locator {
            path.layers_found += 1;
        }

        path.leaves.sort();

        Ok(path)
    }

    async fn write_path(
        &self,
        tx: &mut db::Transaction,
        lock: &mut Lock<'_>,
        path: &PathWithSiblings,
    ) -> Result<RootNode> {
        if path.root.is_null() {
            return self.write_branch_root(tx, lock, &path.root).await;
        }

        for (inner_i, inner_layer) in path.inner.iter().enumerate() {
            let parent_hash = path.hash_at_layer(inner_i);

            for (bucket, ref node) in inner_layer.iter().enumerate() {
                if node.hash.is_null() {
                    continue;
                }

                InnerNode::insert(node, bucket, &parent_hash, tx).await?;
            }
        }

        let layer = PathWithSiblings::total_layer_count() - 1;
        let parent_hash = path.hash_at_layer(layer - 1);

        for leaf in &path.leaves {
            LeafNode::insert(&leaf, &parent_hash, tx).await?;
        }

        self.write_branch_root(tx, lock, &path.root).await
    }

    async fn write_branch_root(
        &self,
        tx: &mut db::Transaction,
        lock: &mut Lock<'_>,
        root: &Hash,
    ) -> Result<RootNode> {
        let new_root = lock.clone_with_new_root(tx, root).await?;
        let old_root = lock.clone();
        **lock = new_root;
        Ok(old_root)
    }

    async fn remove_snapshot(&self, root_node: &RootNode, tx: &mut db::Transaction) -> Result<()> {
        root_node.as_node().remove_recursive(0, tx).await
    }

    async fn lock(&self) -> Lock<'_> {
        self.root_node.lock().await
    }
}

type InnerChildren = [InnerNode; MAX_INNER_NODE_CHILD_COUNT];

#[derive(Debug)]
struct PathWithSiblings {
    encoded_locator: Hash,
    /// Count of the number of layers found where a locator has a corresponding bucket. Including
    /// the root and leaf layers.  (e.g. 0 -> root wasn't found; 1 -> root was found but no inner
    /// nor leaf layers was; 2 -> root and one inner (possibly leaf if INNER_LAYER_COUNT == 0)
    /// layers were found; ...)
    layers_found: usize,
    root: Hash,
    inner: [InnerChildren; INNER_LAYER_COUNT],
    /// Note: this vector must be sorted to guarantee unique hashing.
    //leaves: Vec<(LocatorHash, BlockId)>,
    leaves: Vec<LeafNode>,
}

impl PathWithSiblings {
    fn new(root: &Hash, encoded_locator: Hash) -> Self {
        let null_hash = Hash::null();

        let inner = [
            [InnerNode{hash: null_hash}; MAX_INNER_NODE_CHILD_COUNT];
            INNER_LAYER_COUNT
        ];

        Self {
            encoded_locator,
            layers_found: 0,
            root: *root,
            inner,
            leaves: Vec::new(),
        }
    }

    fn get_leaf(&self, encoded_locator: &LocatorHash) -> Option<BlockId> {
        self.leaves
            .iter()
            .find(|l| l.locator == *encoded_locator)
            .map(|l| l.block_id)
    }

    fn has_leaf(&self, block_id: &BlockId) -> bool {
        self.leaves.iter().any(|l| l.block_id == *block_id)
    }

    fn total_layer_count() -> usize {
        1 /* root */ + INNER_LAYER_COUNT + 1 /* leaves */
    }

    fn hash_at_layer(&self, layer: usize) -> Hash {
        if layer == 0 {
            return self.root;
        }
        let inner_layer = layer - 1;
        self.inner[inner_layer][self.get_bucket(inner_layer)].hash
    }

    // BlockVersion is needed when calculating hashes at the beginning to make this tree unique
    // across all the branches.
    fn insert_leaf(&mut self, block_id: &BlockId) {
        if self.has_leaf(block_id) {
            return;
        }

        let mut modified = false;

        for leaf in &mut self.leaves {
            if leaf.locator == self.encoded_locator {
                modified = true;
                leaf.block_id = *block_id;
                break;
            }
        }

        if !modified {
            // XXX: This can be done better.
            self.leaves.push(LeafNode {
                locator: self.encoded_locator,
                block_id: *block_id,
            });
            self.leaves.sort();
        }

        self.recalculate(INNER_LAYER_COUNT);
    }

    fn remove_leaf(&mut self, encoded_locator: &LocatorHash) {
        let mut changed = false;

        self.leaves = self
            .leaves
            .iter()
            .filter(|l| {
                let keep = l.locator != *encoded_locator;
                if !keep {
                    changed = true;
                }
                keep
            })
            .cloned()
            .collect();

        if !changed {
            return;
        }

        if !self.leaves.is_empty() {
            self.recalculate(INNER_LAYER_COUNT);
            return;
        }

        if INNER_LAYER_COUNT > 0 {
            self.remove_from_inner_layer(INNER_LAYER_COUNT - 1);
        } else {
            self.remove_root_layer();
        }
    }

    fn remove_from_inner_layer(&mut self, inner_layer: usize) {
        let null = Hash::null();
        let bucket = self.get_bucket(inner_layer);

        self.inner[inner_layer][bucket] = InnerNode{hash: null};

        let is_empty = self.inner[inner_layer].iter().all(|x| x.hash == null);

        if !is_empty {
            self.recalculate(inner_layer - 1);
            return;
        }

        if inner_layer > 0 {
            self.remove_from_inner_layer(inner_layer - 1);
        } else {
            self.remove_root_layer();
        }
    }

    fn remove_root_layer(&mut self) {
        self.root = Hash::null();
    }

    /// Recalculate layers from start_layer all the way to the root.
    fn recalculate(&mut self, start_layer: usize) {
        for inner_layer in (0..start_layer).rev() {
            let hash = self.compute_hash_for_layer(inner_layer + 1);
            self.inner[inner_layer][self.get_bucket(inner_layer)] = InnerNode{hash};
        }

        self.root = self.compute_hash_for_layer(0);
    }

    // Assumes layers higher than `layer` have their hashes/BlockVersions already
    // computed/assigned.
    fn compute_hash_for_layer(&self, layer: usize) -> Hash {
        if layer == INNER_LAYER_COUNT {
            hash_leafs(&self.leaves)
        } else {
            hash_inner(&self.inner[layer])
        }
    }

    fn get_bucket(&self, inner_layer: usize) -> usize {
        self.encoded_locator.as_ref()[inner_layer] as usize
    }
}

fn hash_leafs(leaves: &[LeafNode]) -> Hash {
    let mut hash = Sha3_256::new();
    // XXX: Is updating with length enough to prevent attaks?
    hash.update((leaves.len() as u32).to_le_bytes());
    for ref l in leaves {
        hash.update(l.locator);
        hash.update(l.block_id.name);
        hash.update(l.block_id.version);
    }
    hash.finalize().into()
}

fn hash_inner(siblings: &[InnerNode]) -> Hash {
    // XXX: Have some cryptographer check this whether there are no attacks.
    let mut hash = Sha3_256::new();
    for (k, ref s) in siblings.iter().enumerate() {
        if !s.hash.is_null() {
            hash.update((k as u16).to_le_bytes());
            hash.update(s.hash);
        }
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::Cryptor, index, locator::Locator};

    #[tokio::test(flavor = "multi_thread")]
    async fn insert_and_read() {
        let pool = init_db().await;
        let branch = Branch::new(pool.clone(), ReplicaId::random())
            .await
            .unwrap();
        let block_id = BlockId::random();
        let locator = Locator::Head(block_id.name, 0);
        let encoded_locator = locator.encode(&Cryptor::Null).unwrap();

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
            let branch = Branch::new(pool.clone(), ReplicaId::random())
                .await
                .unwrap();

            let b1 = BlockId::random();
            let b2 = BlockId::random();

            let locator = Locator::Head(b1.name, 0);

            let encoded_locator = locator.encode(&Cryptor::Null).unwrap();

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
        let branch = Branch::new(pool.clone(), ReplicaId::random())
            .await
            .unwrap();

        let b = BlockId::random();

        let locator = Locator::Head(b.name, 0);

        let encoded_locator = locator.encode(&Cryptor::Null).unwrap();

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
                Err(Error::BlockIdNotFound) => { /* OK */ }
                Err(_) => {
                    panic!("Error should have been BlockIdNotFound");
                }
                Ok(_) => {
                    panic!("Branch shouldn't have contained the block ID");
                }
            }
        }

        assert_eq!(0, count_branch_forest_entries(&mut tx).await);
    }

    async fn count_branch_forest_entries(tx: &mut db::Transaction) -> usize {
        sqlx::query("select 0 from branch_forest")
            .fetch_all(&mut *tx)
            .await
            .unwrap()
            .len()
    }

    async fn init_db() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        pool
    }
}

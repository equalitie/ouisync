// This is temporary to avoid lint errors when INNER_LAYER_COUNT = 0
#![allow(clippy::reversed_empty_ranges)]

use crate::{
    block::{BlockId, BlockName, BlockVersion},
    crypto::Hash,
    db,
    error::{Error, Result},
    replica_id::ReplicaId,
};
use async_recursion::async_recursion;
use sha3::{Digest, Sha3_256};
use sqlx::{sqlite::SqliteRow, Row};
use std::{convert::TryFrom, sync::Arc};
use tokio::sync::{Mutex, MutexGuard};

/// Number of layers in the tree excluding the layer with root and the layer with leaf nodes.
const INNER_LAYER_COUNT: usize = 1;
const MAX_INNER_NODE_CHILD_COUNT: usize = 256; // = sizeof(u8)

type SnapshotId = u32;

type Lock<'a> = MutexGuard<'a, State>;

struct State {
    snapshot_id: SnapshotId,
    branch_root: Hash,
}

pub struct Branch {
    state: Arc<Mutex<State>>,
    replica_id: ReplicaId,
}

impl Branch {
    pub async fn new(pool: db::Pool, replica_id: ReplicaId) -> Result<Self> {
        let mut conn = pool.acquire().await?;

        let (snapshot_id, branch_root) = match sqlx::query(
            "SELECT snapshot_id, branch_root FROM branches WHERE replica_id=? ORDER BY snapshot_id DESC LIMIT 1",
        )
        .bind(replica_id.as_ref())
        .fetch_optional(&mut conn)
        .await?
        {
            Some(row) => {
                (row.get(0), column::<Hash>(&row, 1)?)
            },
            None => {
                let snapshot_id = sqlx::query(
                    "INSERT INTO branches(replica_id, branch_root)
                             VALUES (?, ?) RETURNING snapshot_id;",
                )
                .bind(replica_id.as_ref())
                .bind(Hash::null().as_ref())
                .fetch_optional(&mut conn)
                .await?
                .unwrap()
                .get(0);

                (snapshot_id, Hash::null())
            }
        };

        Ok(Self {
            state: Arc::new(Mutex::new(State { snapshot_id, branch_root })),
            replica_id,
        })
    }

    pub fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
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

        let mut path = self.get_path(tx, &lock.branch_root, &encoded_locator).await?;

        // We shouldn't be inserting a block to a branch twice. If we do, the assumption is that we
        // hit one in 2^sizeof(BlockVersion) chance that we randomly generated the same
        // BlockVersion twice.
        assert!(!path.has_leaf(block_id));

        path.insert_leaf(&block_id);
        self.write_path(tx, &mut lock, &path).await?;

        Ok(())
    }

    /// Insert the root block into the index
    pub async fn insert_root(&self, tx: &mut db::Transaction, block_id: &BlockId) -> Result<()> {
        self.insert(tx, block_id, &Hash::null()).await
    }

    /// Retrieve `BlockId` of a block with the given encoded `Locator`.
    pub async fn get(&self, tx: &mut db::Transaction, encoded_locator: &Hash) -> Result<BlockId> {
        let lock = self.lock().await;

        if lock.branch_root.is_null() {
            return Err(Error::BlockIdNotFound);
        }

        let path = self.get_path(tx, &lock.branch_root, &encoded_locator).await?;

        match path.get_leaf(encoded_locator) {
            Some(block_id) => Ok(block_id),
            None => Err(Error::BlockIdNotFound),
        }
    }

    /// Get the root block from the index.
    pub async fn get_root(&self, tx: &mut db::Transaction) -> Result<BlockId> {
        self.get(tx, &Hash::null()).await
    }

    async fn get_path(
        &self,
        tx: &mut db::Transaction,
        branch_root: &Hash,
        encoded_locator: &LocatorHash,
    ) -> Result<PathWithSiblings> {
        let mut path = PathWithSiblings::new(&branch_root, *encoded_locator);

        if path.root.is_null() {
            return Ok(path);
        }

        path.layers_found += 1;

        let mut parent = path.root;

        for level in 0..INNER_LAYER_COUNT {
            let children = sqlx::query("SELECT bucket, node FROM branch_forest WHERE parent = ?")
                .bind(parent.as_ref())
                .fetch_all(&mut *tx)
                .await?;

            for ref row in children {
                let bucket: u32 = row.get(0);
                let node = column::<Hash>(row, 1)?;
                path.inner[level][bucket as usize] = node;
            }

            parent = path.inner[level][path.get_bucket(level)];

            if parent.is_null() {
                return Ok(path);
            }

            path.layers_found += 1;
        }

        let children = sqlx::query("SELECT node FROM branch_forest WHERE parent = ?")
            .bind(parent.as_ref())
            .fetch_all(&mut *tx)
            .await?;

        path.leafs.reserve(children.len());

        let mut found_leaf = false;

        for ref row in children {
            let (locator, block_id) = deserialize_leaf(row.get(0))?;
            if *encoded_locator == locator {
                found_leaf = true;
            }
            path.leafs.push((locator, block_id));
        }

        if found_leaf {
            path.layers_found += 1;
        }

        path.leafs.sort();

        Ok(path)
    }

    async fn write_path(
        &self,
        tx: &mut db::Transaction,
        lock: &mut Lock<'_>,
        path: &PathWithSiblings,
    ) -> Result<()> {
        for (inner_i, inner_layer) in path.inner.iter().enumerate() {
            let parent_hash = path.hash_at_layer(inner_i);

            for (bucket, ref hash) in inner_layer.iter().enumerate() {
                // XXX: It should be possible to insert multiple rows at once.
                sqlx::query("INSERT INTO branch_forest (parent, bucket, node) VALUES (?, ?, ?)")
                    .bind(parent_hash.as_ref())
                    .bind(bucket as u16)
                    .bind(hash.as_ref())
                    .execute(&mut *tx)
                    .await?;
            }
        }

        let layer = PathWithSiblings::total_layer_count() - 1;
        let parent_hash = path.hash_at_layer(layer - 1);

        for (ref l, ref block_id) in &path.leafs {
            let blob = serialize_leaf(l, block_id);

            sqlx::query("INSERT INTO branch_forest (parent, bucket, node) VALUES (?, ?, ?)")
                .bind(parent_hash.as_ref())
                .bind(u16::MAX)
                .bind(blob)
                .execute(&mut *tx)
                .await?;
        }

        self.write_branch_root(tx, lock, &path.root).await?;

        Ok(())
    }

    async fn write_branch_root(
        &self,
        tx: &mut db::Transaction,
        lock: &mut Lock<'_>,
        root: &Hash,
    ) -> Result<()> {
        let new_id = sqlx::query(
            "INSERT INTO branches(replica_id, branch_root)
             SELECT replica_id, ? FROM branches
             WHERE snapshot_id=? RETURNING snapshot_id;",
        )
        .bind(root.as_ref())
        .bind(lock.snapshot_id)
        .fetch_optional(&mut *tx)
        .await
        .unwrap()
        .unwrap()
        .get(0);

        self.remove_branch(lock.snapshot_id, root, tx).await?;
        lock.snapshot_id = new_id;
        lock.branch_root = *root;

        Ok(())
    }

    async fn remove_branch(
        &self,
        snapshot_id: SnapshotId,
        root: &Hash,
        tx: &mut db::Transaction,
    ) -> Result<()> {
        BranchNode::Root {
            root: *root,
            snapshot_id,
        }
        .remove_recursive(tx)
        .await
    }

    async fn lock(&self) -> Lock<'_> {
        self.state.lock().await
    }
}

enum BranchNode {
    Root {
        root: Hash,
        snapshot_id: SnapshotId,
    },
    Inner {
        node: Hash,
        parent: Hash,
    },
    Leaf {
        node: (LocatorHash, BlockId),
        parent: Hash,
    },
}

impl BranchNode {
    #[async_recursion]
    async fn remove_recursive(&self, tx: &mut db::Transaction) -> Result<()> {
        self.remove_single(tx).await?;

        if self.is_dangling(tx).await? {
            return Ok(());
        }

        for child in self.children(tx).await? {
            child.remove_recursive(tx).await?;
        }

        Ok(())
    }

    async fn remove_single(&self, tx: &mut db::Transaction) -> Result<()> {
        match self {
            BranchNode::Root {
                root: _,
                snapshot_id,
            } => {
                sqlx::query("DELETE FROM branches WHERE snapshot_id = ?")
                    .bind(snapshot_id)
                    .execute(&mut *tx)
                    .await?;
            }
            BranchNode::Inner { node, parent } => {
                sqlx::query("DELETE FROM branch_forest WHERE parent = ?, node = ?")
                    .bind(parent.as_ref())
                    .bind(node.as_ref())
                    .execute(&mut *tx)
                    .await?;
            }
            BranchNode::Leaf { node, parent } => {
                let blob = serialize_leaf(&node.0, &node.1);
                sqlx::query("DELETE FROM branch_forest WHERE parent = ?, node = ?")
                    .bind(parent.as_ref())
                    .bind(blob)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        Ok(())
    }

    /// Return true if there is nothing that references this node
    async fn is_dangling(&self, tx: &mut db::Transaction) -> Result<bool> {
        let r = match self {
            BranchNode::Root {
                root,
                snapshot_id: _,
            } => sqlx::query("SELECT 0 FROM branches WHERE branch_root = ? LIMIT 1")
                .bind(root.as_ref())
                .fetch_optional(&mut *tx)
                .await?
                .is_some(),
            BranchNode::Inner { node, parent } => {
                sqlx::query("SELECT 0 FROM branch_forest WHERE parent=?, node=? LIMIT 1")
                    .bind(parent.as_ref())
                    .bind(node.as_ref())
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            BranchNode::Leaf { node, parent } => {
                let blob = serialize_leaf(&node.0, &node.1);
                sqlx::query("SELECT 0 FROM branch_forest WHERE parent=?, node=? LIMIT 1")
                    .bind(parent.as_ref())
                    .bind(blob)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
        };

        Ok(r)
    }

    async fn children(&self, tx: &mut db::Transaction) -> Result<Vec<BranchNode>> {
        match self {
            BranchNode::Root {
                root,
                snapshot_id: _,
            } => sqlx::query("SELECT node, parent FROM branch_root WHERE parent=?;")
                .bind(root.as_ref())
                .fetch_all(&mut *tx)
                .await?
                .iter()
                .map(|row| {
                    if INNER_LAYER_COUNT > 0 {
                        Self::row_to_inner(row)
                    } else {
                        Self::row_to_leaf(row)
                    }
                })
                .collect(),
            BranchNode::Inner { node, parent: _ } => {
                sqlx::query("SELECT node, parent FROM branch_forest WHERE parent=?;")
                    .bind(node.as_ref())
                    .fetch_all(&mut *tx)
                    .await?
                    .iter()
                    .map(Self::row_to_leaf)
                    .collect()
            }
            BranchNode::Leaf { node: _, parent: _ } => {
                unimplemented!();
            }
        }
    }

    fn row_to_inner(row: &SqliteRow) -> Result<BranchNode> {
        Ok(BranchNode::Inner {
            node: column::<Hash>(row, 0)?,
            parent: column::<Hash>(row, 1)?,
        })
    }

    fn row_to_leaf(row: &SqliteRow) -> Result<BranchNode> {
        Ok(BranchNode::Leaf {
            node: deserialize_leaf(row.get(0))?,
            parent: column::<Hash>(row, 1)?,
        })
    }
}

fn serialize_leaf(locator: &Hash, block_id: &BlockId) -> Vec<u8> {
    locator
        .as_ref()
        .iter()
        .chain(block_id.name.as_ref().iter())
        .chain(block_id.version.as_ref().iter())
        .cloned()
        .collect()
}

fn deserialize_leaf(blob: &[u8]) -> Result<(LocatorHash, BlockId)> {
    let (b1, b2) = blob.split_at(std::mem::size_of::<Hash>());
    let (b2, b3) = b2.split_at(std::mem::size_of::<BlockName>());
    let l = Hash::try_from(b1)?;
    let name = BlockName::try_from(b2)?;
    let version = BlockVersion::try_from(b3)?;
    Ok((l, BlockId { name, version }))
}

type InnerChildren = [Hash; MAX_INNER_NODE_CHILD_COUNT];
type LocatorHash = Hash;

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
    leafs: Vec<(LocatorHash, BlockId)>,
}

impl PathWithSiblings {
    fn new(root: &Hash, encoded_locator: Hash) -> Self {
        Self {
            encoded_locator,
            layers_found: 0,
            root: *root,
            inner: [[Hash::null(); MAX_INNER_NODE_CHILD_COUNT]; INNER_LAYER_COUNT], //Default::default(),
            leafs: Vec::new(),
        }
    }

    fn get_leaf(&self, encoded_locator: &LocatorHash) -> Option<BlockId> {
        self.leafs
            .iter()
            .find(|(ref l, ref _v)| l == encoded_locator)
            .map(|p| p.1)
    }

    fn has_leaf(&self, block_id: &BlockId) -> bool {
        self.leafs.iter().any(|(_l, id)| id == block_id)
    }

    // Found root and all inner nodes.
    fn has_complete_path(&self) -> bool {
        self.layers_found > INNER_LAYER_COUNT
    }

    fn total_layer_count() -> usize {
        1 /* root */ + INNER_LAYER_COUNT + 1 /* leafs */
    }

    fn hash_at_layer(&self, layer: usize) -> Hash {
        if layer == 0 {
            return self.root;
        }
        let inner_layer = layer - 1;
        self.inner[inner_layer][self.get_bucket(inner_layer)]
    }

    // BlockVersion is needed when calculating hashes at the beginning to make this tree unique
    // across all the branches.
    fn insert_leaf(&mut self, block_id: &BlockId) {
        if self.has_leaf(block_id) {
            return;
        }

        let mut modified = false;

        for leaf in &mut self.leafs {
            if leaf.0 == self.encoded_locator {
                modified = true;
                leaf.1 = *block_id;
                break;
            }
        }

        if !modified {
            // XXX: This can be done better.
            self.leafs.push((self.encoded_locator, *block_id));
            self.leafs.sort();
        }

        for inner_layer in (0..INNER_LAYER_COUNT).rev() {
            let hash = self.compute_hash_for_layer(inner_layer + 1);
            self.inner[inner_layer][self.get_bucket(inner_layer)] = hash;
        }

        self.root = self.compute_hash_for_layer(0);
    }

    // Assumes layers higher than `layer` have their hashes/BlockVersions already
    // computed/assigned.
    fn compute_hash_for_layer(&self, layer: usize) -> Hash {
        if layer == INNER_LAYER_COUNT {
            hash_leafs(&self.leafs)
        } else {
            hash_inner(&self.inner[layer])
        }
    }

    fn get_bucket(&self, inner_layer: usize) -> usize {
        self.encoded_locator.as_ref()[inner_layer] as usize
    }
}

fn hash_leafs(leafs: &[(LocatorHash, BlockId)]) -> Hash {
    let mut hash = Sha3_256::new();
    // XXX: Is updating with length enough to prevent attaks?
    hash.update((leafs.len() as u32).to_le_bytes());
    for (ref l, ref id) in leafs {
        hash.update(l);
        hash.update(id.name);
        hash.update(id.version);
    }
    hash.finalize().into()
}

fn hash_inner(siblings: &[Hash]) -> Hash {
    // XXX: Have some cryptographer check this whether there are no attacks.
    let mut hash = Sha3_256::new();
    for (k, ref s) in siblings.iter().enumerate() {
        if !s.is_null() {
            hash.update((k as u16).to_le_bytes());
            hash.update(s);
        }
    }
    hash.finalize().into()
}

fn column<'a, T: TryFrom<&'a [u8]>>(
    row: &'a SqliteRow,
    i: usize,
) -> std::result::Result<T, T::Error> {
    let value: &'a [u8] = row.get::<'a>(i);
    let value = T::try_from(value)?;
    Ok(value)
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
        }
    }

    async fn init_db() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        index::init(&pool).await.unwrap();
        pool
    }
}
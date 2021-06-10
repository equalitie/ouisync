use super::{SnapshotId, INNER_LAYER_COUNT};
use crate::{
    block::BlockId,
    crypto::{Hash, Hashable},
    db,
    error::Result,
    replica_id::ReplicaId,
};
use async_recursion::async_recursion;
use futures::{future, Stream, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use sqlx::Row;
use std::{
    collections::{btree_map, BTreeMap},
    convert::TryInto,
    iter::FromIterator,
    mem, slice, vec,
};

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct RootNode {
    pub snapshot_id: SnapshotId,
    pub hash: Hash,
}

impl RootNode {
    /// Returns the latest root node of the specified replica. If no such node exists yet, creates
    /// it first.
    pub async fn load_latest_or_create(
        tx: &mut db::Transaction,
        replica_id: &ReplicaId,
    ) -> Result<Self> {
        let node = Self::load_all(&mut *tx, replica_id, 1).try_next().await?;

        if let Some(node) = node {
            Ok(node)
        } else {
            Ok(Self::create(tx, replica_id, InnerNodeMap::default().hash())
                .await?
                .0)
        }
    }

    /// Creates a root node of the specified replica. Returns the node itself and a flag indicating
    /// whether a new node was created (`true`) or the node already existed (`false`).
    pub async fn create(
        tx: &mut db::Transaction,
        replica_id: &ReplicaId,
        hash: Hash,
    ) -> Result<(Self, bool)> {
        let row = sqlx::query(
            "INSERT INTO snapshot_root_nodes (replica_id, hash)
             VALUES (?, ?)
             ON CONFLICT (replica_id, hash) DO NOTHING;
             SELECT snapshot_id, CHANGES()
             FROM snapshot_root_nodes
             WHERE replica_id = ? AND hash = ?",
        )
        .bind(replica_id)
        .bind(&hash)
        .bind(replica_id)
        .bind(&hash)
        .fetch_one(tx)
        .await?;

        Ok((
            RootNode {
                snapshot_id: row.get(0),
                hash,
            },
            row.get::<u32, _>(1) > 0,
        ))
    }

    /// Returns a stream of all (but at most `limit`) root nodes corresponding to the specified
    /// replica ordered from the most recent to the least recent.
    pub fn load_all<'a>(
        tx: &'a mut db::Transaction,
        replica_id: &'a ReplicaId,
        limit: u32,
    ) -> impl Stream<Item = Result<Self>> + 'a {
        sqlx::query(
            "SELECT snapshot_id, hash
             FROM snapshot_root_nodes
             WHERE replica_id = ?
             ORDER BY snapshot_id DESC
             LIMIT ?",
        )
        .bind(replica_id)
        .bind(limit)
        .map(|row| Self {
            snapshot_id: row.get(0),
            hash: row.get(1),
        })
        .fetch(tx)
        .err_into()
    }

    pub async fn clone_with_new_hash(&self, tx: &mut db::Transaction, hash: Hash) -> Result<Self> {
        let snapshot_id = sqlx::query(
            "INSERT INTO snapshot_root_nodes (replica_id, hash)
             SELECT replica_id, ?
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?
             RETURNING snapshot_id;",
        )
        .bind(&hash)
        .bind(self.snapshot_id)
        .fetch_one(tx)
        .await?
        .get(0);

        Ok(Self { snapshot_id, hash })
    }

    /// Check whether all the constituent nodes of this snapshot have been downloaded and stored
    /// locally.
    pub async fn check_complete(&self, tx: &mut db::Transaction) -> Result<bool> {
        check_subtree_complete(tx, &self.hash, 0).await
    }

    pub async fn remove_recursive(&self, tx: &mut db::Transaction) -> Result<()> {
        self.as_link().remove_recursive(0, tx).await
    }

    fn as_link(&self) -> Link {
        Link::ToRoot { node: self.clone() }
    }
}

// TODO: find a way to avoid the recursion
#[async_recursion]
async fn check_subtree_complete(
    tx: &mut db::Transaction,
    parent_hash: &Hash,
    layer: usize,
) -> Result<bool> {
    if layer < INNER_LAYER_COUNT {
        let nodes = InnerNode::load_children(tx, parent_hash).await?;
        let mut complete_children_count = 0usize;

        for (_, node) in &nodes {
            if check_subtree_complete(tx, &node.hash, layer + 1).await? {
                complete_children_count += 1;
            }
        }

        Ok(nodes.hash() == *parent_hash && complete_children_count == nodes.len())
    } else {
        let nodes = LeafNode::load_children(tx, parent_hash).await?;
        Ok(nodes.hash() == *parent_hash)
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct InnerNode {
    pub hash: Hash,
}

impl InnerNode {
    /// Saves this inner node into the db. Returns whether a new node was created (`true`) or the
    /// node already existed (`false`).
    pub async fn save(&self, tx: &mut db::Transaction, parent: &Hash, bucket: u8) -> Result<bool> {
        let changes = sqlx::query(
            "INSERT INTO snapshot_inner_nodes (parent, bucket, hash)
             VALUES (?, ?, ?)
             ON CONFLICT (parent, bucket) DO NOTHING",
        )
        .bind(parent)
        .bind(bucket)
        .bind(&self.hash)
        .execute(tx)
        .await?
        .rows_affected();

        Ok(changes > 0)
    }

    pub async fn load_children(tx: &mut db::Transaction, parent: &Hash) -> Result<InnerNodeMap> {
        sqlx::query(
            "SELECT bucket, hash
             FROM snapshot_inner_nodes
             WHERE parent = ?",
        )
        .bind(parent)
        .map(|row| {
            let bucket: u32 = row.get(0);
            let node = Self { hash: row.get(1) };

            (bucket, node)
        })
        .fetch(tx)
        .try_filter_map(|(bucket, node)| {
            // TODO: consider reporting out-of-range buckets as errors
            future::ready(Ok(bucket.try_into().ok().map(|bucket| (bucket, node))))
        })
        .try_collect()
        .await
        .map_err(From::from)
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct InnerNodeMap(BTreeMap<u8, InnerNode>);

impl InnerNodeMap {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, bucket: u8) -> Option<&InnerNode> {
        self.0.get(&bucket)
    }

    pub fn iter(&self) -> InnerNodeMapIter {
        InnerNodeMapIter(self.0.iter())
    }

    pub fn insert(&mut self, bucket: u8, node: InnerNode) -> Option<InnerNode> {
        self.0.insert(bucket, node)
    }

    pub fn remove(&mut self, bucket: u8) -> Option<InnerNode> {
        self.0.remove(&bucket)
    }
}

impl Extend<(u8, InnerNode)> for InnerNodeMap {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = (u8, InnerNode)>,
    {
        self.0.extend(iter)
    }
}

impl IntoIterator for InnerNodeMap {
    type Item = (u8, InnerNode);
    type IntoIter = btree_map::IntoIter<u8, InnerNode>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a InnerNodeMap {
    type Item = (u8, &'a InnerNode);
    type IntoIter = InnerNodeMapIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl Hashable for InnerNodeMap {
    fn hash(&self) -> Hash {
        // XXX: Have some cryptographer check this whether there are no attacks.
        let mut hasher = Sha3_256::new();
        for (bucket, node) in self.iter() {
            hasher.update(bucket.to_le_bytes());
            hasher.update(node.hash);
        }
        hasher.finalize().into()
    }
}

pub struct InnerNodeMapIter<'a>(btree_map::Iter<'a, u8, InnerNode>);

impl<'a> Iterator for InnerNodeMapIter<'a> {
    type Item = (u8, &'a InnerNode);

    fn next(&mut self) -> Option<(u8, &'a InnerNode)> {
        self.0.next().map(|(bucket, node)| (*bucket, node))
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct LeafNode {
    locator: Hash,
    pub block_id: BlockId,
}

impl LeafNode {
    pub fn locator(&self) -> &Hash {
        &self.locator
    }

    pub async fn save(&self, tx: &mut db::Transaction, parent: &Hash) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_leaf_nodes (parent, locator, block_id)
             VALUES (?, ?, ?)",
        )
        .bind(parent)
        .bind(&self.locator)
        .bind(&self.block_id)
        .execute(tx)
        .await?;

        Ok(())
    }

    pub async fn load_children(tx: &mut db::Transaction, parent: &Hash) -> Result<LeafNodeSet> {
        Ok(sqlx::query(
            "SELECT locator, block_id
             FROM snapshot_leaf_nodes
             WHERE parent = ?",
        )
        .bind(parent)
        .map(|row| LeafNode {
            locator: row.get(0),
            block_id: row.get(1),
        })
        .fetch_all(tx)
        .await?
        .into_iter()
        .collect())
    }
}

/// Collection that acts as a ordered set of `LeafNode`s
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct LeafNodeSet(Vec<LeafNode>);

impl LeafNodeSet {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, locator: &Hash) -> Option<&LeafNode> {
        self.lookup(locator).ok().map(|index| &self.0[index])
    }

    pub fn iter(&self) -> impl Iterator<Item = &LeafNode> {
        self.0.iter()
    }

    /// Inserts a new node or updates it if already exists.
    pub fn modify(&mut self, locator: &Hash, block_id: &BlockId) -> ModifyStatus {
        match self.lookup(locator) {
            Ok(index) => {
                let node = &mut self.0[index];

                if &node.block_id == block_id {
                    ModifyStatus::Unchanged
                } else {
                    ModifyStatus::Updated(mem::replace(&mut node.block_id, *block_id))
                }
            }
            Err(index) => {
                self.0.insert(
                    index,
                    LeafNode {
                        locator: *locator,
                        block_id: *block_id,
                    },
                );
                ModifyStatus::Inserted
            }
        }
    }

    pub fn remove(&mut self, locator: &Hash) -> Option<LeafNode> {
        let index = self.lookup(locator).ok()?;
        Some(self.0.remove(index))
    }

    fn lookup(&self, locator: &Hash) -> Result<usize, usize> {
        self.0.binary_search_by(|node| node.locator.cmp(locator))
    }
}

impl FromIterator<LeafNode> for LeafNodeSet {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = LeafNode>,
    {
        let mut vec: Vec<_> = iter.into_iter().collect();
        vec.sort_by(|lhs, rhs| lhs.locator.cmp(&rhs.locator));

        Self(vec)
    }
}

impl<'a> IntoIterator for &'a LeafNodeSet {
    type Item = &'a LeafNode;
    type IntoIter = slice::Iter<'a, LeafNode>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl IntoIterator for LeafNodeSet {
    type Item = LeafNode;
    type IntoIter = vec::IntoIter<LeafNode>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl Hashable for LeafNodeSet {
    fn hash(&self) -> Hash {
        let mut hasher = Sha3_256::new();
        // XXX: Is updating with length enough to prevent attacks?
        hasher.update((self.len() as u32).to_le_bytes());
        for node in self.iter() {
            hasher.update(node.locator());
            hasher.update(node.block_id);
        }
        hasher.finalize().into()
    }
}

pub enum ModifyStatus {
    Updated(BlockId),
    Inserted,
    Unchanged,
}

// We're not repeating enumeration name
// https://rust-lang.github.io/rust-clippy/master/index.html#enum_variant_names
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
enum Link {
    ToRoot { node: RootNode },
    ToInner { parent: Hash, node: InnerNode },
    ToLeaf { parent: Hash, node: LeafNode },
}

impl Link {
    #[async_recursion]
    pub async fn remove_recursive(&self, layer: usize, tx: &mut db::Transaction) -> Result<()> {
        self.remove_single(tx).await?;

        if !self.is_dangling(tx).await? {
            return Ok(());
        }

        for child in self.children(layer, tx).await? {
            child.remove_recursive(layer + 1, tx).await?;
        }

        Ok(())
    }

    async fn remove_single(&self, tx: &mut db::Transaction) -> Result<()> {
        match self {
            Link::ToRoot { node } => {
                sqlx::query("DELETE FROM snapshot_root_nodes WHERE snapshot_id = ?")
                    .bind(node.snapshot_id)
                    .execute(&mut *tx)
                    .await?;
            }
            Link::ToInner { parent, node } => {
                sqlx::query("DELETE FROM snapshot_inner_nodes WHERE parent = ? AND hash = ?")
                    .bind(parent)
                    .bind(&node.hash)
                    .execute(&mut *tx)
                    .await?;
            }
            Link::ToLeaf { parent, node } => {
                sqlx::query("DELETE FROM snapshot_leaf_nodes WHERE parent = ? AND locator = ? AND block_id = ?")
                    .bind(parent)
                    .bind(node.locator())
                    .bind(&node.block_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        Ok(())
    }

    /// Return true if there is nothing that references this node
    async fn is_dangling(&self, tx: &mut db::Transaction) -> Result<bool> {
        let has_parent = match self {
            Link::ToRoot { node: root } => {
                sqlx::query("SELECT 0 FROM snapshot_root_nodes WHERE hash = ? LIMIT 1")
                    .bind(&root.hash)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Link::ToInner { parent: _, node } => {
                sqlx::query("SELECT 0 FROM snapshot_inner_nodes WHERE hash = ? LIMIT 1")
                    .bind(&node.hash)
                    .fetch_optional(&mut *tx)
                    .await?
                    .is_some()
            }
            Link::ToLeaf { parent: _, node } => sqlx::query(
                "SELECT 0
                 FROM snapshot_leaf_nodes
                 WHERE locator = ? AND block_id = ?
                 LIMIT 1",
            )
            .bind(node.locator())
            .bind(&node.block_id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some(),
        };

        Ok(!has_parent)
    }

    async fn children(&self, layer: usize, tx: &mut db::Transaction) -> Result<Vec<Link>> {
        match self {
            Link::ToRoot { node: root } => self.inner_children(tx, &root.hash).await,
            Link::ToInner { node, .. } if layer < INNER_LAYER_COUNT => {
                self.inner_children(tx, &node.hash).await
            }
            Link::ToInner { node, .. } => self.leaf_children(tx, &node.hash).await,
            Link::ToLeaf { parent: _, node: _ } => Ok(Vec::new()),
        }
    }

    async fn inner_children(&self, tx: &mut db::Transaction, parent: &Hash) -> Result<Vec<Link>> {
        sqlx::query(
            "SELECT parent, hash
             FROM snapshot_inner_nodes
             WHERE parent = ?;",
        )
        .bind(parent)
        .map(|row| Link::ToInner {
            parent: row.get(0),
            node: InnerNode { hash: row.get(1) },
        })
        .fetch(tx)
        .try_collect()
        .await
        .map_err(From::from)
    }

    async fn leaf_children(&self, tx: &mut db::Transaction, parent: &Hash) -> Result<Vec<Link>> {
        sqlx::query(
            "SELECT parent, locator, block_id
             FROM snapshot_leaf_nodes
             WHERE parent = ?;",
        )
        .bind(parent)
        .map(|row| Link::ToLeaf {
            parent: row.get(0),
            node: LeafNode {
                locator: row.get(1),
                block_id: row.get(2),
            },
        })
        .fetch(tx)
        .try_collect()
        .await
        .map_err(From::from)
    }
}

/// Get the bucket for `locator` at the specified `inner_layer`.
pub fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{crypto::Hashable, test_utils};
    use assert_matches::assert_matches;
    use rand::prelude::*;
    use std::collections::HashMap;
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn create_new_root_node() {
        let pool = setup().await;

        let replica_id = rand::random();
        let hash = rand::random::<u64>().hash();

        let mut tx = pool.begin().await.unwrap();
        let (node0, changed) = RootNode::create(&mut tx, &replica_id, hash).await.unwrap();
        assert!(changed);
        assert_eq!(node0.hash, hash);

        let node1 = RootNode::load_latest_or_create(&mut tx, &replica_id)
            .await
            .unwrap();
        assert_eq!(node1, node0);

        let nodes: Vec<_> = RootNode::load_all(&mut tx, &replica_id, 2)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], node0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_existing_root_node() {
        let pool = setup().await;

        let replica_id = rand::random();
        let hash = rand::random::<u64>().hash();

        let mut tx = pool.begin().await.unwrap();
        let (node0, _) = RootNode::create(&mut tx, &replica_id, hash).await.unwrap();

        let (node1, changed) = RootNode::create(&mut tx, &replica_id, hash).await.unwrap();
        assert_eq!(node0, node1);
        assert!(!changed);

        let nodes: Vec<_> = RootNode::load_all(&mut tx, &replica_id, 2)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0], node0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_new_inner_node() {
        let pool = setup().await;

        let parent = rand::random::<u64>().hash();
        let hash = rand::random::<u64>().hash();
        let bucket = rand::random();

        let mut tx = pool.begin().await.unwrap();

        let node = InnerNode { hash };
        assert!(node.save(&mut tx, &parent, bucket).await.unwrap());

        let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();

        assert_eq!(nodes.get(bucket), Some(&node));

        assert!((0..bucket).all(|b| nodes.get(b).is_none()));
        assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_existing_inner_node() {
        let pool = setup().await;

        let parent = rand::random::<u64>().hash();
        let hash = rand::random::<u64>().hash();
        let bucket = rand::random();

        let mut tx = pool.begin().await.unwrap();

        let node0 = InnerNode { hash };
        node0.save(&mut tx, &parent, bucket).await.unwrap();

        let node1 = InnerNode { hash };
        assert!(!node1.save(&mut tx, &parent, bucket).await.unwrap());

        let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();

        assert_eq!(nodes.get(bucket), Some(&node0));
        assert!((0..bucket).all(|b| nodes.get(b).is_none()));
        assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_conflicting_existing_inner_node() {
        let pool = setup().await;

        let parent = rand::random::<u64>().hash();
        let bucket = rand::random();

        let hash0 = rand::random::<u64>().hash();
        let hash1 = loop {
            let hash = rand::random::<u64>().hash();
            if hash != hash0 {
                break hash;
            }
        };

        let mut tx = pool.begin().await.unwrap();

        let node0 = InnerNode { hash: hash0 };
        node0.save(&mut tx, &parent, bucket).await.unwrap();

        let node1 = InnerNode { hash: hash1 };
        assert_matches!(node1.save(&mut tx, &parent, bucket).await, Err(_)); // TODO: match concrete error type
    }

    #[proptest]
    fn check_complete(
        #[strategy(0usize..=32)] leaf_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        test_utils::run(check_complete_case(leaf_count, rng_seed))
    }

    async fn check_complete_case(leaf_count: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);

        let pool = setup().await;
        let mut tx = pool.begin().await.unwrap();

        let snapshot = Snapshot::generate(&mut rng, leaf_count);

        let (root_node, _) = RootNode::create(&mut tx, &snapshot.replica_id, snapshot.root_hash)
            .await
            .unwrap();

        if leaf_count > 0 {
            assert!(!root_node.check_complete(&mut tx).await.unwrap());
        }

        // TODO: consider randomizing the order the nodes are saved so it's not always
        // breadth-first.

        for (inner_layer, maps) in snapshot.inners.iter().enumerate() {
            for (path, nodes) in maps {
                let parent_hash = snapshot.parent_hash(inner_layer, path);

                for (bucket, node) in nodes {
                    node.save(&mut tx, parent_hash, bucket).await.unwrap();
                    assert!(!root_node.check_complete(&mut tx).await.unwrap());
                }
            }
        }

        let mut unsaved_leaves: usize = snapshot.leaves.values().map(|nodes| nodes.len()).sum();

        for (path, nodes) in &snapshot.leaves {
            let parent_hash = snapshot.parent_hash(INNER_LAYER_COUNT, path);

            for node in nodes {
                node.save(&mut tx, parent_hash).await.unwrap();
                unsaved_leaves -= 1;
                if unsaved_leaves > 0 {
                    assert!(!root_node.check_complete(&mut tx).await.unwrap());
                }
            }
        }

        assert!(root_node.check_complete(&mut tx).await.unwrap());
    }

    async fn setup() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        super::super::init(&pool).await.unwrap();
        pool
    }

    // In-memory snapshot for testing purposes.
    struct Snapshot {
        replica_id: ReplicaId,
        root_hash: Hash,
        inners: [HashMap<BucketPath, InnerNodeMap>; INNER_LAYER_COUNT],
        leaves: HashMap<BucketPath, LeafNodeSet>,
    }

    impl Snapshot {
        // Generate a random snapshot with the given maximum number of leaf nodes.
        fn generate<R: Rng>(rng: &mut R, leaf_count: usize) -> Self {
            let replica_id = rng.gen();
            let leaves = (0..leaf_count)
                .map(|_| {
                    let locator = rng.gen::<u64>().hash();
                    let block_id = rng.gen();
                    LeafNode { locator, block_id }
                })
                .collect();

            Self::build(replica_id, leaves)
        }

        fn build(replica_id: ReplicaId, leaves: Vec<LeafNode>) -> Self {
            let leaves =
                leaves
                    .into_iter()
                    .fold(HashMap::<_, LeafNodeSet>::new(), |mut map, leaf| {
                        map.entry(BucketPath::new(&leaf.locator, INNER_LAYER_COUNT - 1))
                            .or_default()
                            .modify(&leaf.locator, &leaf.block_id);
                        map
                    });

            let mut inners: [HashMap<_, InnerNodeMap>; INNER_LAYER_COUNT] = Default::default();

            for (path, set) in &leaves {
                add_inner_node(
                    INNER_LAYER_COUNT - 1,
                    &mut inners[INNER_LAYER_COUNT - 1],
                    path,
                    set.hash(),
                );
            }

            for layer in (0..INNER_LAYER_COUNT - 1).rev() {
                let (lo, hi) = inners.split_at_mut(layer + 1);

                for (path, map) in &hi[0] {
                    add_inner_node(layer, lo.last_mut().unwrap(), path, map.hash());
                }
            }

            let root_hash = inners[0]
                .get(&BucketPath::default())
                .unwrap_or(&InnerNodeMap::default())
                .hash();

            Self {
                replica_id,
                root_hash,
                inners,
                leaves,
            }
        }

        // Returns the parent hash of inner nodes at `inner_layer` with the specified bucket path.
        fn parent_hash(&self, inner_layer: usize, path: &BucketPath) -> &Hash {
            if inner_layer == 0 {
                &self.root_hash
            } else {
                let (bucket, parent_path) = path.pop(inner_layer - 1);
                &self.inners[inner_layer - 1]
                    .get(&parent_path)
                    .unwrap()
                    .get(bucket)
                    .unwrap()
                    .hash
            }
        }
    }

    fn add_inner_node(
        inner_layer: usize,
        maps: &mut HashMap<BucketPath, InnerNodeMap>,
        path: &BucketPath,
        hash: Hash,
    ) {
        let (bucket, parent_path) = path.pop(inner_layer);
        maps.entry(parent_path)
            .or_default()
            .insert(bucket, InnerNode { hash });
    }

    #[derive(Default, Clone, Copy, Eq, PartialEq, Hash, Debug)]
    struct BucketPath([u8; INNER_LAYER_COUNT]);

    impl BucketPath {
        fn new(locator: &Hash, inner_layer: usize) -> Self {
            let mut path = Self(Default::default());
            for (layer, bucket) in path.0.iter_mut().enumerate().take(inner_layer + 1) {
                *bucket = get_bucket(locator, layer)
            }
            path
        }

        fn pop(&self, inner_layer: usize) -> (u8, Self) {
            let mut popped = *self;
            let bucket = mem::replace(&mut popped.0[inner_layer], 0);
            (bucket, popped)
        }
    }
}

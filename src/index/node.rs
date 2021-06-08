use super::{SnapshotId, INNER_LAYER_COUNT};
use crate::{block::BlockId, crypto::Hash, db, error::Result, replica_id::ReplicaId};
use async_recursion::async_recursion;
use futures::{future, Stream, TryStreamExt};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{collections::BTreeMap, convert::TryInto, iter::FromIterator, mem, slice};

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
            Ok(Self::create(tx, replica_id, Hash::null()).await?.0)
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

    pub async fn remove_recursive(&self, tx: &mut db::Transaction) -> Result<()> {
        self.as_link().remove_recursive(0, tx).await
    }

    fn as_link(&self) -> Link {
        Link::ToRoot { node: self.clone() }
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
            future::ready(Ok(bucket.try_into().ok().map(|bucket| (bucket, node))))
        })
        .try_collect()
        .await
        .map_err(From::from)
    }
}

impl Default for InnerNode {
    fn default() -> Self {
        Self { hash: Hash::null() }
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct InnerNodeMap(BTreeMap<u8, InnerNode>);

impl InnerNodeMap {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, index: u8) -> Option<&InnerNode> {
        self.0.get(&index)
    }

    pub fn contains(&self, index: u8) -> bool {
        self.0.contains_key(&index)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u8, &InnerNode)> {
        self.0.iter().map(|(index, node)| (*index, node))
    }

    pub fn into_iter(self) -> impl Iterator<Item = (u8, InnerNode)> {
        self.0.into_iter()
    }

    pub fn insert(&mut self, index: u8, node: InnerNode) -> Option<InnerNode> {
        self.0.insert(index, node)
    }

    pub fn remove(&mut self, index: u8) -> Option<InnerNode> {
        self.0.remove(&index)
    }
}

// impl FromIterator<InnerNode> for InnerNodeMap {
//     fn from_iter<T>(iter: T) -> Self
//     where
//         T: IntoIterator<Item = InnerNode>,
//     {
//         Self(
//             iter.into_iter()
//                 .enumerate()
//                 .filter_map(|(index, node)| index.try_into().ok().map(|index| (index, node)))
//                 .collect(),
//         )
//     }
// }

impl Extend<(u8, InnerNode)> for InnerNodeMap {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = (u8, InnerNode)>,
    {
        self.0.extend(iter)
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

    pub async fn insert(&self, parent: &Hash, tx: &mut db::Transaction) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_leaf_nodes (parent, locator, block_id)
             VALUES (?, ?, ?)",
        )
        .bind(parent)
        .bind(&self.locator)
        .bind(&self.block_id)
        .execute(&mut *tx)
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

    pub fn get(&self, locator: &Hash) -> Option<&LeafNode> {
        self.lookup(locator).ok().map(|index| &self.0[index])
    }

    pub fn iter(&self) -> slice::Iter<LeafNode> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Hashable;
    use assert_matches::assert_matches;

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
        assert!((0..bucket).all(|b| !nodes.contains(b)));
        assert!((bucket + 1..).all(|b| !nodes.contains(b)));
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
        assert!((0..bucket).all(|b| !nodes.contains(b)));
        assert!((bucket + 1..).all(|b| !nodes.contains(b)));
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

    // TODO: test saving InnerNode requires parent to exists

    async fn setup() -> db::Pool {
        let pool = db::Pool::connect(":memory:").await.unwrap();
        super::super::init(&pool).await.unwrap();
        pool
    }
}

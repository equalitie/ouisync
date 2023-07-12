use super::error::Error;
use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, Digest, Hash, Hashable},
    db,
    index::SingleBlockPresence,
};
use futures_util::{Stream, TryStreamExt};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{iter::FromIterator, slice, vec};

#[cfg(test)]
use {
    super::inner_node::{self, INNER_LAYER_COUNT},
    async_recursion::async_recursion,
};

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct LeafNode {
    pub locator: Hash,
    pub block_id: BlockId,
    pub block_presence: SingleBlockPresence,
}

#[derive(Default)]
pub(crate) struct ReceiveStatus {
    /// Whether any of the snapshots were already approved.
    pub old_approved: bool,
    /// List of branches whose snapshots have been approved.
    pub new_approved: Vec<PublicKey>,
    /// Which of the received nodes should we request the blocks of.
    pub request_blocks: Vec<LeafNode>,
}

pub(super) async fn load_children(
    conn: &mut db::Connection,
    parent: &Hash,
) -> Result<LeafNodeSet, Error> {
    Ok(sqlx::query(
        "SELECT locator, block_id, block_presence
         FROM snapshot_leaf_nodes
         WHERE parent = ?",
    )
    .bind(parent)
    .fetch(conn)
    .map_ok(|row| LeafNode {
        locator: row.get(0),
        block_id: row.get(1),
        block_presence: row.get(2),
    })
    .try_collect::<Vec<_>>()
    .await?
    .into_iter()
    .collect())
}

pub(super) fn load_parent_hashes<'a>(
    conn: &'a mut db::Connection,
    block_id: &'a BlockId,
) -> impl Stream<Item = Result<Hash, Error>> + 'a {
    sqlx::query("SELECT DISTINCT parent FROM snapshot_leaf_nodes WHERE block_id = ?")
        .bind(block_id)
        .fetch(conn)
        .map_ok(|row| row.get(0))
        .err_into()
}

/// Loads all locators (most of the time (always?) there will be at most one) pointing to the
/// block id.
pub(super) fn load_locators<'a>(
    conn: &'a mut db::Connection,
    block_id: &'a BlockId,
) -> impl Stream<Item = Result<Hash, Error>> + 'a {
    sqlx::query("SELECT locator FROM snapshot_leaf_nodes WHERE block_id = ?")
        .bind(block_id)
        .fetch(conn)
        .map_ok(|row| row.get(0))
        .err_into()
}

/// Saves the node to the db unless it already exists.
pub(super) async fn save(
    tx: &mut db::WriteTransaction,
    node: &LeafNode,
    parent: &Hash,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO snapshot_leaf_nodes (parent, locator, block_id, block_presence)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (parent, locator, block_id) DO NOTHING",
    )
    .bind(parent)
    .bind(&node.locator)
    .bind(&node.block_id)
    .bind(node.block_presence)
    .execute(tx)
    .await?;

    Ok(())
}

pub(super) async fn save_all(
    tx: &mut db::WriteTransaction,
    nodes: &LeafNodeSet,
    parent: &Hash,
) -> Result<(), Error> {
    for node in nodes {
        save(tx, node, parent).await?;
    }

    Ok(())
}

/// Checks whether the block with the specified id is present.
pub(super) async fn is_present(
    conn: &mut db::Connection,
    block_id: &BlockId,
) -> Result<bool, Error> {
    Ok(
        sqlx::query("SELECT 1 FROM snapshot_leaf_nodes WHERE block_id = ? AND block_presence = ?")
            .bind(block_id)
            .bind(SingleBlockPresence::Present)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// Marks all leaf nodes that point to the specified block as present (not missing). Returns
/// whether at least one node was modified.
pub(super) async fn set_present(
    tx: &mut db::WriteTransaction,
    block_id: &BlockId,
) -> Result<bool, Error> {
    // Check whether there is at least one node that references the given block.
    if sqlx::query("SELECT 1 FROM snapshot_leaf_nodes WHERE block_id = ? LIMIT 1")
        .bind(block_id)
        .fetch_optional(&mut *tx)
        .await?
        .is_none()
    {
        return Err(Error::BlockNotReferenced);
    }

    // Update only those nodes that have block_presence set to `Missing`.
    let result = sqlx::query(
        "UPDATE snapshot_leaf_nodes SET block_presence = ? WHERE block_id = ? AND block_presence = ?",
        )
        .bind(SingleBlockPresence::Present)
        .bind(block_id)
        .bind(SingleBlockPresence::Missing)
        .execute(tx)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Marks all leaf nodes that point to the specified block as missing.
pub(super) async fn set_missing(
    tx: &mut db::WriteTransaction,
    block_id: &BlockId,
) -> Result<(), Error> {
    sqlx::query("UPDATE snapshot_leaf_nodes SET block_presence = ? WHERE block_id = ?")
        .bind(SingleBlockPresence::Missing)
        .bind(block_id)
        .execute(tx)
        .await?;

    Ok(())
}

// Filter nodes that the remote replica has a block for but the local one is missing it.
pub(super) async fn filter_nodes_with_new_blocks(
    conn: &mut db::Connection,
    remote_nodes: &LeafNodeSet,
) -> Result<Vec<LeafNode>, Error> {
    let mut output = Vec::new();

    for remote_node in remote_nodes.present() {
        if !is_present(conn, &remote_node.block_id).await? {
            output.push(*remote_node);
        }
    }

    Ok(output)
}

pub(super) async fn count(conn: &mut db::Connection) -> Result<u64, Error> {
    Ok(db::decode_u64(
        sqlx::query("SELECT COUNT(*) FROM snapshot_leaf_nodes")
            .fetch_one(conn)
            .await?
            .get(0),
    ))
}

#[cfg(test)]
#[async_recursion]
pub(super) async fn count_in(
    conn: &mut db::Connection,
    current_layer: usize,
    node: &Hash,
) -> Result<usize, Error> {
    // TODO: this can be rewritten as a single query using CTE

    if current_layer < INNER_LAYER_COUNT {
        let children = inner_node::load_children(conn, node).await?;

        let mut sum = 0;

        for (_bucket, child) in children {
            sum += count_in(conn, current_layer + 1, &child.hash).await?;
        }

        Ok(sum)
    } else {
        Ok(load_children(conn, node).await?.len())
    }
}

impl LeafNode {
    /// Creates a leaf node whose block is assumed to be present (not missing) in this replica.
    /// (currently test-only).
    #[cfg(test)]
    pub fn present(locator: Hash, block_id: BlockId) -> Self {
        Self {
            locator,
            block_id,
            block_presence: SingleBlockPresence::Present,
        }
    }

    /// Creates a leaf node whose block is assumed to be missing in this replica
    /// (currently test-only).
    #[cfg(test)]
    pub fn missing(locator: Hash, block_id: BlockId) -> Self {
        Self {
            locator,
            block_id,
            block_presence: SingleBlockPresence::Missing,
        }
    }
}

impl Hashable for LeafNode {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.locator.update_hash(state);
        self.block_id.update_hash(state);
    }
}

/// Collection that acts as a ordered set of `LeafNode`s
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LeafNodeSet(Vec<LeafNode>);

impl LeafNodeSet {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[allow(unused)]
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
    pub fn modify(
        &mut self,
        locator: &Hash,
        block_id: &BlockId,
        block_presence: SingleBlockPresence,
    ) -> ModifyStatus {
        match self.lookup(locator) {
            Ok(index) => {
                let node = &mut self.0[index];

                if &node.block_id == block_id {
                    ModifyStatus::Unchanged
                } else {
                    let old_block_id = node.block_id;
                    node.block_id = *block_id;
                    node.block_presence = block_presence;

                    ModifyStatus::Updated(old_block_id)
                }
            }
            Err(index) => {
                self.0.insert(
                    index,
                    LeafNode {
                        locator: *locator,
                        block_id: *block_id,
                        block_presence,
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

    /// Returns the same nodes but with the `block_presence` set to `Missing`.
    /// Equivalent to `self.into_iter().map(LeafNode::into_missing()).collect()` but without
    /// involving reallocation.
    pub fn into_missing(mut self) -> Self {
        for node in &mut self.0 {
            node.block_presence = SingleBlockPresence::Missing;
        }

        self
    }

    /// Returns all nodes from this set whose `block_presence` is `Present`.
    pub fn present(&self) -> impl Iterator<Item = &LeafNode> {
        self.iter()
            .filter(|node| node.block_presence == SingleBlockPresence::Present)
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
    fn update_hash<S: Digest>(&self, state: &mut S) {
        b"leaf".update_hash(state); // to disambiguate it from hash of inner nodes
        self.0.update_hash(state);
    }
}

// Cached hash of an empty LeafNodeSet.
pub(crate) static EMPTY_LEAF_HASH: Lazy<Hash> = Lazy::new(|| LeafNodeSet::default().hash());

pub(crate) enum ModifyStatus {
    Updated(BlockId),
    Inserted,
    Unchanged,
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn save_new_present() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::present(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Present);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn save_new_missing() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Missing);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn save_missing_node_over_existing_missing_one() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Missing);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn save_missing_node_over_existing_present_one() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::present(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(nodes.len(), 1);

        let node = nodes.get(&encoded_locator).unwrap();
        assert_eq!(node.locator, encoded_locator);
        assert_eq!(node.block_id, block_id);
        assert_eq!(node.block_presence, SingleBlockPresence::Present);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_present_on_node_with_missing_block() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::missing(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        assert!(set_present(&mut tx, &block_id).await.unwrap());

        let nodes = load_children(&mut tx, &parent).await.unwrap();
        assert_eq!(
            nodes.get(&encoded_locator).unwrap().block_presence,
            SingleBlockPresence::Present
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_present_on_node_with_present_block() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let encoded_locator = rand::random();
        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = LeafNode::present(encoded_locator, block_id);
        save(&mut tx, &node, &parent).await.unwrap();

        assert!(!set_present(&mut tx, &block_id).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_present_on_node_that_does_not_exist() {
        let (_base_dir, pool) = setup().await;

        let block_id = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        assert_matches!(
            set_present(&mut tx, &block_id).await,
            Err(Error::BlockNotReferenced)
        )
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}

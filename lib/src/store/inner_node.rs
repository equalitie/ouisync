use super::{
    error::Error,
    leaf_node::{self, LeafNodeSet, EMPTY_LEAF_HASH},
    ReceiveFilter,
};
use crate::{
    crypto::{sign::PublicKey, Digest, Hash, Hashable},
    db,
    index::Summary,
};
use futures_util::{future, Stream, TryStreamExt};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{
    collections::{btree_map, BTreeMap},
    convert::TryInto,
    iter::FromIterator,
};

/// Number of layers in the tree excluding the layer with root and the layer with leaf nodes.
pub(crate) const INNER_LAYER_COUNT: usize = 3;

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct InnerNode {
    pub hash: Hash,
    pub summary: Summary,
}

#[derive(Default)]
pub(crate) struct ReceiveStatus {
    /// List of branches whose snapshots have been approved.
    pub new_approved: Vec<PublicKey>,
    /// Which of the received nodes should we request the children of.
    pub request_children: Vec<InnerNode>,
}

/// Load all inner nodes with the specified parent hash.
pub(super) async fn load_children(
    conn: &mut db::Connection,
    parent: &Hash,
) -> Result<InnerNodeMap, Error> {
    sqlx::query(
        "SELECT
             bucket,
             hash,
             state,
             block_presence
         FROM snapshot_inner_nodes
         WHERE parent = ?",
    )
    .bind(parent)
    .fetch(conn)
    .map_ok(|row| {
        let bucket: u32 = row.get(0);
        let node = InnerNode {
            hash: row.get(1),
            summary: Summary {
                state: row.get(2),
                block_presence: row.get(3),
            },
        };

        (bucket, node)
    })
    .try_filter_map(|(bucket, node)| {
        if let Ok(bucket) = bucket.try_into() {
            future::ready(Ok(Some((bucket, node))))
        } else {
            tracing::error!("bucket out of range: {:?}", bucket);
            future::ready(Ok(None))
        }
    })
    .try_collect()
    .await
    .map_err(From::from)
}

pub(super) async fn load(
    conn: &mut db::Connection,
    hash: &Hash,
) -> Result<Option<InnerNode>, Error> {
    let node = sqlx::query(
        "SELECT state, block_presence
         FROM snapshot_inner_nodes
         WHERE hash = ?",
    )
    .bind(hash)
    .fetch_optional(conn)
    .await?
    .map(|row| InnerNode {
        hash: *hash,
        summary: Summary {
            state: row.get(0),
            block_presence: row.get(1),
        },
    });

    Ok(node)
}

/// Saves this inner node into the db unless it already exists.
pub(super) async fn save(
    tx: &mut db::WriteTransaction,
    node: &InnerNode,
    parent: &Hash,
    bucket: u8,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO snapshot_inner_nodes (
             parent,
             bucket,
             hash,
             state,
             block_presence
         )
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (parent, bucket) DO NOTHING",
    )
    .bind(parent)
    .bind(bucket)
    .bind(&node.hash)
    .bind(node.summary.state)
    .bind(&node.summary.block_presence)
    .execute(tx)
    .await?;

    Ok(())
}

/// Atomically saves all nodes in this map to the db.
pub(super) async fn save_all(
    tx: &mut db::WriteTransaction,
    nodes: &InnerNodeMap,
    parent: &Hash,
) -> Result<(), Error> {
    for (bucket, node) in nodes {
        save(tx, node, parent, bucket).await?;
    }

    Ok(())
}

/// Compute summaries from the children nodes of the specified parent nodes.
pub(super) async fn compute_summary(
    conn: &mut db::Connection,
    parent_hash: &Hash,
) -> Result<Summary, Error> {
    // 1st attempt: empty inner nodes
    if parent_hash == &*EMPTY_INNER_HASH {
        let children = InnerNodeMap::default();
        return Ok(Summary::from_inners(&children));
    }

    // 2nd attempt: empty leaf nodes
    if parent_hash == &*EMPTY_LEAF_HASH {
        let children = LeafNodeSet::default();
        return Ok(Summary::from_leaves(&children));
    }

    // 3rd attempt: non-empty inner nodes
    let children = load_children(conn, parent_hash).await?;
    if !children.is_empty() {
        // We download all children nodes of a given parent together so when we know that
        // we have at least one we also know we have them all.
        return Ok(Summary::from_inners(&children));
    }

    // 4th attempt: non-empty leaf nodes
    let children = leaf_node::load_children(conn, parent_hash).await?;
    if !children.is_empty() {
        // Similarly as in the inner nodes case, we only need to check that we have at
        // least one leaf node child and that already tells us that we have them all.
        return Ok(Summary::from_leaves(&children));
    }

    // The parent hash doesn't correspond to any known node
    Ok(Summary::INCOMPLETE)
}

/// Updates summaries of all nodes with the specified hash at the specified inner layer.
pub(super) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    hash: &Hash,
) -> Result<(), Error> {
    let summary = compute_summary(tx, hash).await?;

    sqlx::query(
        "UPDATE snapshot_inner_nodes
         SET state = ?, block_presence = ?
         WHERE hash = ?",
    )
    .bind(summary.state)
    .bind(&summary.block_presence)
    .bind(hash)
    .execute(tx)
    .await?;

    Ok(())
}

pub(super) async fn inherit_summaries(
    conn: &mut db::Connection,
    nodes: &mut InnerNodeMap,
) -> Result<(), Error> {
    for (_, node) in nodes {
        inherit_summary(conn, node).await?;
    }

    Ok(())
}

/// If the summary of this node is `INCOMPLETE` and there exists another node with the same
/// hash as this one, copy the summary of that node into this node.
///
/// Note this is hack/workaround due to the database schema currently not being fully
/// normalized. That is, when there is a node that has more than one parent, we actually
/// represent it as multiple records, each with distinct parent_hash. The summaries of those
/// records need to be identical (because they conceptually represent a single node) so we need
/// this function to copy them manually.
/// Ideally, we should change the db schema to be normalized, which in this case would mean
/// to have only one record per node and to represent the parent-child relation using a
/// separate db table (many-to-many relation).
async fn inherit_summary(conn: &mut db::Connection, node: &mut InnerNode) -> Result<(), Error> {
    if node.summary != Summary::INCOMPLETE {
        return Ok(());
    }

    let summary = sqlx::query(
        "SELECT state, block_presence
             FROM snapshot_inner_nodes
             WHERE hash = ?",
    )
    .bind(&node.hash)
    .fetch_optional(conn)
    .await?
    .map(|row| Summary {
        state: row.get(0),
        block_presence: row.get(1),
    });

    if let Some(summary) = summary {
        node.summary = summary;
    }

    Ok(())
}

/// Filter nodes that the remote replica has some blocks in that the local one is missing.
pub(super) async fn filter_nodes_with_new_blocks(
    tx: &mut db::WriteTransaction,
    remote_nodes: &InnerNodeMap,
    receive_filter: &ReceiveFilter,
) -> Result<Vec<InnerNode>, Error> {
    let mut output = Vec::with_capacity(remote_nodes.len());

    for (_, remote_node) in remote_nodes {
        if !receive_filter
            .check(tx, &remote_node.hash, &remote_node.summary.block_presence)
            .await?
        {
            continue;
        }

        let local_node = load(tx, &remote_node.hash).await?;
        let insert = if let Some(local_node) = local_node {
            local_node.summary.is_outdated(&remote_node.summary)
        } else {
            // node not present locally - we implicitly treat this as if the local replica
            // had zero blocks under this node unless the remote node is empty, in that
            // case we ignore it.
            !remote_node.is_empty()
        };

        if insert {
            output.push(*remote_node);
        }
    }

    Ok(output)
}

impl InnerNode {
    /// Creates new unsaved inner node with the specified hash.
    pub fn new(hash: Hash, summary: Summary) -> Self {
        Self { hash, summary }
    }

    /// Loads parent hashes of all inner nodes with the specifed hash.
    #[deprecated]
    pub fn load_parent_hashes<'a>(
        conn: &'a mut db::Connection,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<Hash, Error>> + 'a {
        sqlx::query("SELECT DISTINCT parent FROM snapshot_inner_nodes WHERE hash = ?")
            .bind(hash)
            .fetch(conn)
            .map_ok(|row| row.get(0))
            .err_into()
    }

    /// Updates summaries of all nodes with the specified hash at the specified inner layer.
    #[deprecated]
    pub async fn update_summaries(tx: &mut db::WriteTransaction, hash: &Hash) -> Result<(), Error> {
        update_summaries(tx, hash).await
    }

    pub fn is_empty(&self) -> bool {
        self.hash == *EMPTY_INNER_HASH || self.hash == *EMPTY_LEAF_HASH
    }
}

impl Hashable for InnerNode {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        self.hash.update_hash(state);
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct InnerNodeMap(BTreeMap<u8, InnerNode>);

impl InnerNodeMap {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[allow(unused)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, bucket: u8) -> Option<&InnerNode> {
        self.0.get(&bucket)
    }

    pub fn iter(&self) -> InnerNodeMapIter {
        InnerNodeMapIter(self.0.iter())
    }

    pub fn iter_mut(&mut self) -> InnerNodeMapIterMut {
        InnerNodeMapIterMut(self.0.iter_mut())
    }

    pub fn insert(&mut self, bucket: u8, node: InnerNode) -> Option<InnerNode> {
        self.0.insert(bucket, node)
    }

    pub fn remove(&mut self, bucket: u8) -> Option<InnerNode> {
        self.0.remove(&bucket)
    }

    /// Returns the same nodes but with the `state` and `block_presence` fields changed to
    /// indicate that these nodes are not complete yet.
    pub fn into_incomplete(mut self) -> Self {
        for node in self.0.values_mut() {
            node.summary = Summary::INCOMPLETE;
        }

        self
    }
}

impl FromIterator<(u8, InnerNode)> for InnerNodeMap {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = (u8, InnerNode)>,
    {
        Self(iter.into_iter().collect())
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

impl<'a> IntoIterator for &'a mut InnerNodeMap {
    type Item = (u8, &'a mut InnerNode);
    type IntoIter = InnerNodeMapIterMut<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl Hashable for InnerNodeMap {
    fn update_hash<S: Digest>(&self, state: &mut S) {
        b"inner".update_hash(state); // to disambiguate it from hash of leaf nodes
        self.0.update_hash(state);
    }
}

// Cached hash of an empty InnerNodeMap.
pub(crate) static EMPTY_INNER_HASH: Lazy<Hash> = Lazy::new(|| InnerNodeMap::default().hash());

pub(crate) struct InnerNodeMapIter<'a>(btree_map::Iter<'a, u8, InnerNode>);

impl<'a> Iterator for InnerNodeMapIter<'a> {
    type Item = (u8, &'a InnerNode);

    fn next(&mut self) -> Option<(u8, &'a InnerNode)> {
        self.0.next().map(|(bucket, node)| (*bucket, node))
    }
}

pub(crate) struct InnerNodeMapIterMut<'a>(btree_map::IterMut<'a, u8, InnerNode>);

impl<'a> Iterator for InnerNodeMapIterMut<'a> {
    type Item = (u8, &'a mut InnerNode);

    fn next(&mut self) -> Option<(u8, &'a mut InnerNode)> {
        self.0.next().map(|(bucket, node)| (*bucket, node))
    }
}

/// Get the bucket for `locator` at the specified `inner_layer`.
pub(crate) fn get_bucket(locator: &Hash, inner_layer: usize) -> u8 {
    locator.as_ref()[inner_layer]
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    #[test]
    fn empty_map_hash() {
        assert_eq!(*EMPTY_INNER_HASH, InnerNodeMap::default().hash())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_new_inner_node() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let hash = rand::random();
        let bucket = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node = InnerNode::new(hash, Summary::INCOMPLETE);
        save(&mut tx, &node, &parent, bucket).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();

        assert_eq!(nodes.get(bucket), Some(&node));

        assert!((0..bucket).all(|b| nodes.get(b).is_none()));

        if bucket < u8::MAX {
            assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_existing_inner_node() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let hash = rand::random();
        let bucket = rand::random();

        let mut tx = pool.begin_write().await.unwrap();

        let node0 = InnerNode::new(hash, Summary::INCOMPLETE);
        save(&mut tx, &node0, &parent, bucket).await.unwrap();

        let node1 = InnerNode::new(hash, Summary::INCOMPLETE);
        save(&mut tx, &node1, &parent, bucket).await.unwrap();

        let nodes = load_children(&mut tx, &parent).await.unwrap();

        assert_eq!(nodes.get(bucket), Some(&node0));
        assert!((0..bucket).all(|b| nodes.get(b).is_none()));

        if bucket < u8::MAX {
            assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn attempt_to_create_conflicting_node() {
        let (_base_dir, pool) = setup().await;

        let parent = rand::random();
        let bucket = rand::random();

        let hash0 = rand::random();
        let hash1 = loop {
            let hash = rand::random();
            if hash != hash0 {
                break hash;
            }
        };

        let mut tx = pool.begin_write().await.unwrap();

        let node0 = InnerNode::new(hash0, Summary::INCOMPLETE);
        save(&mut tx, &node0, &parent, bucket).await.unwrap();

        let node1 = InnerNode::new(hash1, Summary::INCOMPLETE);
        assert_matches!(save(&mut tx, &node1, &parent, bucket).await, Err(_)); // TODO: match concrete error type
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }
}

use super::{
    leaf::{LeafNode, LeafNodeSet, EMPTY_LEAF_HASH},
    summary::Summary,
};
use crate::{
    crypto::{Digest, Hash, Hashable},
    db,
    error::Result,
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

impl InnerNode {
    /// Creates new unsaved inner node with the specified hash.
    pub fn new(hash: Hash, summary: Summary) -> Self {
        Self { hash, summary }
    }

    /// Load all inner nodes with the specified parent hash.
    pub async fn load_children(conn: &mut db::Connection, parent: &Hash) -> Result<InnerNodeMap> {
        sqlx::query(
            "SELECT
                 bucket,
                 hash,
                 is_complete,
                 block_presence
             FROM snapshot_inner_nodes
             WHERE parent = ?",
        )
        .bind(parent)
        .fetch(conn)
        .map_ok(|row| {
            let bucket: u32 = row.get(0);
            let node = Self {
                hash: row.get(1),
                summary: Summary {
                    is_complete: row.get(2),
                    block_presence: row.get(3),
                },
            };

            (bucket, node)
        })
        .try_filter_map(|(bucket, node)| {
            // TODO: consider reporting out-of-range buckets as errors
            future::ready(Ok(bucket.try_into().ok().map(|bucket| (bucket, node))))
        })
        .try_collect()
        .await
        .map_err(From::from)
    }

    /// Load the inner node with the specified hash
    pub async fn load(conn: &mut db::Connection, hash: &Hash) -> Result<Option<Self>> {
        let node = sqlx::query(
            "SELECT is_complete, block_presence
             FROM snapshot_inner_nodes
             WHERE hash = ?",
        )
        .bind(hash)
        .fetch_optional(conn)
        .await?
        .map(|row| Self {
            hash: *hash,
            summary: Summary {
                is_complete: row.get(0),
                block_presence: row.get(1),
            },
        });

        Ok(node)
    }

    /// Loads parent hashes of all inner nodes with the specifed hash.
    pub fn load_parent_hashes<'a>(
        conn: &'a mut db::Connection,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<Hash>> + 'a {
        sqlx::query("SELECT parent FROM snapshot_inner_nodes WHERE hash = ?")
            .bind(hash)
            .fetch(conn)
            .map_ok(|row| row.get(0))
            .err_into()
    }

    /// Saves this inner node into the db unless it already exists.
    pub async fn save(&self, tx: &mut db::Transaction, parent: &Hash, bucket: u8) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_inner_nodes (
                 parent,
                 bucket,
                 hash,
                 is_complete,
                 block_presence
             )
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (parent, bucket) DO NOTHING",
        )
        .bind(parent)
        .bind(bucket)
        .bind(&self.hash)
        .bind(self.summary.is_complete)
        .bind(&self.summary.block_presence)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Updates summaries of all nodes with the specified hash at the specified inner layer.
    pub async fn update_summaries(tx: &mut db::Transaction, hash: &Hash) -> Result<()> {
        let summary = Self::compute_summary(tx, hash).await?;

        sqlx::query(
            "UPDATE snapshot_inner_nodes
             SET is_complete = ?, block_presence = ?
             WHERE hash = ?",
        )
        .bind(summary.is_complete)
        .bind(&summary.block_presence)
        .bind(hash)
        .execute(&mut **tx)
        .await?;

        Ok(())
    }

    /// Compute summaries from the children nodes of the specified parent nodes.
    pub async fn compute_summary(conn: &mut db::Connection, parent_hash: &Hash) -> Result<Summary> {
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
        let children = InnerNode::load_children(conn, parent_hash).await?;
        if !children.is_empty() {
            // We download all children nodes of a given parent together so when we know that
            // we have at least one we also know we have them all.
            return Ok(Summary::from_inners(&children));
        }

        // 4th attempt: non-empty leaf nodes
        let children = LeafNode::load_children(conn, parent_hash).await?;
        if !children.is_empty() {
            // Similarly as in the inner nodes case, we only need to check that we have at
            // least one leaf node child and that already tells us that we have them all.
            return Ok(Summary::from_leaves(&children));
        }

        // The parent hash doesn't correspond to any known node
        Ok(Summary::INCOMPLETE)
    }

    pub fn is_empty(&self) -> bool {
        self.hash == *EMPTY_INNER_HASH || self.hash == *EMPTY_LEAF_HASH
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
    async fn inherit_summary(&mut self, conn: &mut db::Connection) -> Result<()> {
        if self.summary != Summary::INCOMPLETE {
            return Ok(());
        }

        let summary = sqlx::query(
            "SELECT is_complete, block_presence
             FROM snapshot_inner_nodes
             WHERE hash = ?",
        )
        .bind(&self.hash)
        .fetch_optional(conn)
        .await?
        .map(|row| Summary {
            is_complete: row.get(0),
            block_presence: row.get(1),
        });

        if let Some(summary) = summary {
            self.summary = summary;
        }

        Ok(())
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

    pub fn insert(&mut self, bucket: u8, node: InnerNode) -> Option<InnerNode> {
        self.0.insert(bucket, node)
    }

    pub fn remove(&mut self, bucket: u8) -> Option<InnerNode> {
        self.0.remove(&bucket)
    }

    /// Atomically saves all nodes in this map to the db.
    pub async fn save(&self, tx: &mut db::Transaction, parent: &Hash) -> Result<()> {
        for (bucket, node) in self {
            node.save(tx, parent, bucket).await?;
        }

        Ok(())
    }

    /// Returns the same nodes but with the `is_complete` and `missing_block` fields changed to
    /// indicate that these nodes are not complete yet.
    pub fn into_incomplete(mut self) -> Self {
        for node in self.0.values_mut() {
            node.summary = Summary::INCOMPLETE;
        }

        self
    }

    pub async fn inherit_summaries(&mut self, conn: &mut db::Connection) -> Result<()> {
        for node in self.0.values_mut() {
            node.inherit_summary(conn).await?;
        }

        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_map_hash() {
        assert_eq!(*EMPTY_INNER_HASH, InnerNodeMap::default().hash())
    }
}

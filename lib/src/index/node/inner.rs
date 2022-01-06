use super::{
    leaf::{LeafNode, LeafNodeSet},
    summary::Summary,
};
use crate::{
    crypto::{Hash, Hashable},
    db,
    error::Result,
};
use futures_util::{future, Stream, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use sqlx::{Acquire, Row};
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
    pub fn new(hash: Hash) -> Self {
        Self {
            hash,
            summary: Summary::INCOMPLETE,
        }
    }

    /// Load all inner nodes with the specified parent hash.
    pub async fn load_children(conn: &mut db::Connection, parent: &Hash) -> Result<InnerNodeMap> {
        sqlx::query(
            "SELECT
                 bucket,
                 hash,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             FROM snapshot_inner_nodes
             WHERE parent = ?",
        )
        .bind(parent)
        .map(|row| {
            let bucket: u32 = row.get(0);
            let node = Self {
                hash: row.get(1),
                summary: Summary {
                    is_complete: row.get(2),
                    missing_blocks_count: db::decode_u64(row.get(3)),
                    missing_blocks_checksum: db::decode_u64(row.get(4)),
                },
            };

            (bucket, node)
        })
        .fetch(conn)
        .try_filter_map(|(bucket, node)| {
            // TODO: consider reporting out-of-range buckets as errors
            future::ready(Ok(bucket.try_into().ok().map(|bucket| (bucket, node))))
        })
        .try_collect()
        .await
        .map_err(From::from)
    }

    /// Loads parent hashes of all inner nodes with the specifed hash.
    pub fn load_parent_hashes<'a>(
        conn: &'a mut db::Connection,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<Hash>> + 'a {
        sqlx::query("SELECT parent FROM snapshot_inner_nodes WHERE hash = ?")
            .bind(hash)
            .map(|row| row.get(0))
            .fetch(conn)
            .err_into()
    }

    /// Saves this inner node into the db unless it already exists.
    pub async fn save(
        &self,
        tx: &mut db::Transaction<'_>,
        parent: &Hash,
        bucket: u8,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_inner_nodes (
                 parent,
                 bucket,
                 hash,
                 is_complete,
                 missing_blocks_count,
                 missing_blocks_checksum
             )
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (parent, bucket) DO NOTHING",
        )
        .bind(parent)
        .bind(bucket)
        .bind(&self.hash)
        .bind(self.summary.is_complete)
        .bind(db::encode_u64(self.summary.missing_blocks_count))
        .bind(db::encode_u64(self.summary.missing_blocks_checksum))
        .execute(tx)
        .await?;

        Ok(())
    }

    /// Updates summaries of all nodes with the specified hash at the specified inner layer.
    pub async fn update_summaries(
        tx: &mut db::Transaction<'_>,
        hash: &Hash,
        inner_layer: usize,
    ) -> Result<()> {
        let summary = Self::compute_summary(tx, hash, inner_layer + 1).await?;

        sqlx::query(
            "UPDATE snapshot_inner_nodes
             SET
                 is_complete = ?,
                 missing_blocks_count = ?,
                 missing_blocks_checksum = ?
             WHERE hash = ?",
        )
        .bind(summary.is_complete)
        .bind(db::encode_u64(summary.missing_blocks_count))
        .bind(db::encode_u64(summary.missing_blocks_checksum))
        .bind(hash)
        .execute(tx)
        .await?;

        Ok(())
    }

    /// Compute summaries from the children nodes of the specified parent nodes.
    pub async fn compute_summary(
        conn: &mut db::Connection,
        parent_hash: &Hash,
        parent_layer: usize,
    ) -> Result<Summary> {
        if parent_layer < INNER_LAYER_COUNT {
            let empty_children = InnerNodeMap::default();
            // If the parent hash is equal to the hash of empty node collection it means the node
            // has no children and we can cut this short.
            if *parent_hash == empty_children.hash() {
                Ok(Summary::from_inners(&empty_children))
            } else {
                let children = InnerNode::load_children(&mut *conn, parent_hash).await?;

                // We download all children nodes of a given parent together so when we know that
                // we have at least one we also know we have them all. Thus it's enough to check
                // that all of them are complete.
                if !children.is_empty() {
                    Ok(Summary::from_inners(&children))
                } else {
                    Ok(Summary::INCOMPLETE)
                }
            }
        } else {
            let empty_children = LeafNodeSet::default();

            // If the parent hash is equal to the hash of empty node collection it means the node
            // has no children and we can cut this short.
            if *parent_hash == empty_children.hash() {
                Ok(Summary::from_leaves(&empty_children))
            } else {
                let children = LeafNode::load_children(&mut *conn, parent_hash).await?;

                // Similarly as in the inner nodes case, we only need to check that we have at
                // least one leaf node child and that already tells us that we have them all.
                if !children.is_empty() {
                    Ok(Summary::from_leaves(&children))
                } else {
                    Ok(Summary::INCOMPLETE)
                }
            }
        }
    }
}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub(crate) struct InnerNodeMap(BTreeMap<u8, InnerNode>);

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

    /// Atomically saves all nodes in this map to the db.
    pub async fn save(&self, conn: &'_ mut db::Connection, parent: &'_ Hash) -> Result<()> {
        let mut tx = conn.begin().await?;
        for (bucket, node) in self {
            node.save(&mut tx, parent, bucket).await?;
        }
        tx.commit().await?;

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
    fn hash(&self) -> Hash {
        // XXX: Have some cryptographer check this whether there are no attacks.
        let mut hasher = Sha3_256::new();
        hasher.update(&[self.len() as u8]);
        for (bucket, node) in self.iter() {
            hasher.update(bucket.to_le_bytes());
            hasher.update(node.hash);
        }
        hasher.finalize().into()
    }
}

pub(crate) struct InnerNodeMapIter<'a>(btree_map::Iter<'a, u8, InnerNode>);

impl<'a> Iterator for InnerNodeMapIter<'a> {
    type Item = (u8, &'a InnerNode);

    fn next(&mut self) -> Option<(u8, &'a InnerNode)> {
        self.0.next().map(|(bucket, node)| (*bucket, node))
    }
}

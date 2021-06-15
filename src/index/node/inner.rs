use crate::{
    crypto::{Hash, Hashable},
    db,
    error::Result,
};
use futures::{future, Stream, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use sqlx::Row;
use std::{
    collections::{btree_map, BTreeMap},
    convert::TryInto,
};

/// Number of layers in the tree excluding the layer with root and the layer with leaf nodes.
pub const INNER_LAYER_COUNT: usize = 3;

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct InnerNode {
    pub hash: Hash,
    /// Has the whole subree rooted at this node been completely downloaded?
    ///
    /// Note this is local-only information and is not transmitted to other replicas which is why
    /// it is not serialized.
    #[serde(skip)]
    pub is_complete: bool,
}

impl InnerNode {
    /// Creates new unsaved inner node with the specified hash.
    pub fn new(hash: Hash) -> Self {
        Self {
            hash,
            is_complete: false,
        }
    }

    /// Load all inner nodes with the specified parent hash.
    pub async fn load_children(tx: &mut db::Transaction, parent: &Hash) -> Result<InnerNodeMap> {
        sqlx::query(
            "SELECT bucket, hash, is_complete
             FROM snapshot_inner_nodes
             WHERE parent = ?",
        )
        .bind(parent)
        .map(|row| {
            let bucket: u32 = row.get(0);
            let node = Self {
                hash: row.get(1),
                is_complete: row.get(2),
            };

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

    /// Load parent hashes of all inner nodes with the specifed hash.
    pub fn load_parent_hashes<'a>(
        tx: &'a mut db::Transaction,
        hash: &'a Hash,
    ) -> impl Stream<Item = Result<Hash>> + 'a {
        sqlx::query("SELECT parent FROM snapshot_inner_nodes WHERE hash = ?")
            .bind(hash)
            .map(|row| row.get(0))
            .fetch(tx)
            .err_into()
    }

    /// Set all inner nodes with the specified hash as complete.
    pub async fn set_complete(tx: &mut db::Transaction, hash: &Hash) -> Result<()> {
        sqlx::query("UPDATE snapshot_inner_nodes SET is_complete = 1 WHERE hash = ?")
            .bind(hash)
            .execute(tx)
            .await?;

        Ok(())
    }

    /// Saves this inner node into the db. Returns whether a new node was created (`true`) or the
    /// node already existed (`false`).
    pub async fn save(&self, tx: &mut db::Transaction, parent: &Hash, bucket: u8) -> Result<bool> {
        let changes = sqlx::query(
            "INSERT INTO snapshot_inner_nodes (parent, bucket, hash, is_complete)
             VALUES (?, ?, ?, 0)
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
        hasher.update(&[self.len() as u8]);
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
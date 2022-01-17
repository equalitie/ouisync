use crate::{
    block::BlockId,
    crypto::{Digest, Hash, Hashable},
    db,
    error::Error,
    error::Result,
};
use futures_util::{Stream, TryStreamExt};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sqlx::{Acquire, Row};
use std::{iter::FromIterator, mem, slice, vec};

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct LeafNode {
    locator: Hash,
    pub block_id: BlockId,
    pub is_missing: bool,
}

impl LeafNode {
    /// Creates a leaf node whose block is assumed to be present (not missing) in this replica.
    /// (currently test-only).
    #[cfg(test)]
    pub fn present(locator: Hash, block_id: BlockId) -> Self {
        Self {
            locator,
            block_id,
            is_missing: false,
        }
    }

    /// Creates a leaf node whose block is assumed to be missing in this replica
    /// (currently test-only).
    #[cfg(test)]
    pub fn missing(locator: Hash, block_id: BlockId) -> Self {
        Self {
            locator,
            block_id,
            is_missing: true,
        }
    }

    pub fn locator(&self) -> &Hash {
        &self.locator
    }

    /// Saves the node to the db unless it already exists.
    pub async fn save(&self, conn: &mut db::Connection, parent: &Hash) -> Result<()> {
        sqlx::query(
            "INSERT INTO snapshot_leaf_nodes (parent, locator, block_id, is_missing)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (parent, locator, block_id) DO NOTHING",
        )
        .bind(parent)
        .bind(&self.locator)
        .bind(&self.block_id)
        .bind(&self.is_missing)
        .execute(conn)
        .await?;

        Ok(())
    }

    pub async fn load_children(conn: &mut db::Connection, parent: &Hash) -> Result<LeafNodeSet> {
        Ok(sqlx::query(
            "SELECT locator, block_id, is_missing
             FROM snapshot_leaf_nodes
             WHERE parent = ?",
        )
        .bind(parent)
        .fetch(conn)
        .map_ok(|row| LeafNode {
            locator: row.get(0),
            block_id: row.get(1),
            is_missing: row.get(2),
        })
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .collect())
    }

    /// Loads all parent hashes of nodes with the specified block id.
    pub fn load_parent_hashes<'a>(
        conn: &'a mut db::Connection,
        block_id: &'a BlockId,
    ) -> impl Stream<Item = Result<Hash>> + 'a {
        sqlx::query("SELECT parent FROM snapshot_leaf_nodes WHERE block_id = ?")
            .bind(block_id)
            .fetch(conn)
            .map_ok(|row| row.get(0))
            .err_into()
    }

    /// Marks all leaf nodes that point to the specified block as present (not missing). Returns
    /// whether at least one node was modified.
    pub async fn set_present(conn: &mut db::Connection, block_id: &BlockId) -> Result<bool> {
        // Check whether there is at least one node that references the given block.
        if sqlx::query("SELECT 1 FROM snapshot_leaf_nodes WHERE block_id = ? LIMIT 1")
            .bind(block_id)
            .fetch_optional(&mut *conn)
            .await?
            .is_none()
        {
            return Err(Error::BlockNotReferenced);
        }

        // Update only those nodes that have is_missing set to true.
        let result = sqlx::query(
            "UPDATE snapshot_leaf_nodes SET is_missing = 0 WHERE block_id = ? AND is_missing = 1",
        )
        .bind(block_id)
        .execute(conn)
        .await?;

        Ok(result.rows_affected() > 0)
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
    ///
    /// When a new node is created, it's `is_missing` flag is set to `initial_is_missing`. When an
    /// existing node is update, its `is_missing` flag is left unchanged and `initial_is_missing`
    /// is ignored.
    pub fn modify(
        &mut self,
        locator: &Hash,
        block_id: &BlockId,
        initial_is_missing: bool,
    ) -> ModifyStatus {
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
                        is_missing: initial_is_missing,
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

    pub async fn save(&self, conn: &mut db::Connection, parent: &Hash) -> Result<()> {
        let mut tx = conn.begin().await?;
        for node in self {
            node.save(&mut tx, parent).await?;
        }
        tx.commit().await?;

        Ok(())
    }

    /// Returns the same nodes but with the `is_missing` flag set to `true`.
    /// Equivalent to `self.into_iter().map(LeafNode::into_missing()).collect()` but without
    /// involving reallocation.
    pub fn into_missing(mut self) -> Self {
        for node in &mut self.0 {
            node.is_missing = true;
        }

        self
    }

    /// Returns all nodes from this set whose `is_missing` flag is `false`.
    pub fn present(&self) -> impl Iterator<Item = &LeafNode> {
        self.iter().filter(|node| !node.is_missing)
    }

    /// Returns whether node at `locator` has `is_missing` set to `true` or if there is no such
    /// node in this set.
    pub fn is_missing(&self, locator: &Hash) -> bool {
        self.get(locator)
            .map(|node| node.is_missing)
            .unwrap_or(true)
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

pub enum ModifyStatus {
    Updated(BlockId),
    Inserted,
    Unchanged,
}

use super::SingleBlockPresence;
use crate::{
    block::BlockId,
    crypto::{Digest, Hash, Hashable},
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{slice, vec};

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub(crate) struct LeafNode {
    pub locator: Hash,
    pub block_id: BlockId,
    pub block_presence: SingleBlockPresence,
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
    ) -> LeafNodeModifyStatus {
        match self.lookup(locator) {
            Ok(index) => {
                let node = &mut self.0[index];

                if &node.block_id == block_id {
                    LeafNodeModifyStatus::Unchanged
                } else {
                    let old_block_id = node.block_id;
                    node.block_id = *block_id;
                    node.block_presence = block_presence;

                    LeafNodeModifyStatus::Updated(old_block_id)
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
                LeafNodeModifyStatus::Inserted
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

pub(crate) enum LeafNodeModifyStatus {
    Updated(BlockId),
    Inserted,
    Unchanged,
}

// Cached hash of an empty LeafNodeSet.
pub(crate) static EMPTY_LEAF_HASH: Lazy<Hash> = Lazy::new(|| LeafNodeSet::default().hash());

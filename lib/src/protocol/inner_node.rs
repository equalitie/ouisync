use super::{Summary, EMPTY_LEAF_HASH};
use crate::crypto::{Digest, Hash, Hashable};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::{btree_map, BTreeMap};

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

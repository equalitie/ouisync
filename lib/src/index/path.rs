use super::node::{
    self, InnerNode, InnerNodeMap, LeafNodeSet, ModifyStatus, Summary, INNER_LAYER_COUNT,
};
use crate::{
    block::BlockId,
    crypto::{Hash, Hashable},
};

///
/// Path represents a (possibly incomplete) path in a snapshot from the root to the leaf.
/// Unlike a traditional tree path with only the relevant nodes, this one also contains for each
/// inner layer all siblings of the inner node that would be in the traditional path.
///
/// //                    root
/// //                    /  \
/// //                   a0   a1     |
/// //                  /  \         | inner: [[a0, a1], [b0, b1]]
/// //                 b0   b1       |
/// //                     /  \
/// //                    c0   c1    | leaves: [c0, c1]
///
/// The purpose of this is to be able to modify the path (complete it if it's incomplete, modify
/// and/or remove the leaf) and then recalculate all hashes.
///
#[derive(Debug)]
pub(super) struct Path {
    locator: Hash,
    /// Count of the number of layers found where a locator has a corresponding bucket. Including
    /// the root and leaf layers.  (e.g. 0 -> root wasn't found; 1 -> root was found but no inner
    /// nor leaf layers was; 2 -> root and one inner (possibly leaf if INNER_LAYER_COUNT == 0)
    /// layers were found; ...)
    pub layers_found: usize,
    pub root_hash: Hash,
    pub inner: Vec<InnerNodeMap>,
    pub leaves: LeafNodeSet,
}

impl Path {
    pub fn new(root_hash: Hash, locator: Hash) -> Self {
        let inner = vec![InnerNodeMap::default(); INNER_LAYER_COUNT];

        Self {
            locator,
            layers_found: 0,
            root_hash,
            inner,
            leaves: LeafNodeSet::default(),
        }
    }

    pub fn get_leaf(&self) -> Option<BlockId> {
        self.leaves.get(&self.locator).map(|node| node.block_id)
    }

    pub fn has_leaf(&self, block_id: &BlockId) -> bool {
        self.leaves.iter().any(|l| &l.block_id == block_id)
    }

    pub const fn total_layer_count() -> usize {
        1 /* root */ + INNER_LAYER_COUNT + 1 /* leaves */
    }

    pub fn hash_at_layer(&self, layer: usize) -> Option<Hash> {
        if layer == 0 {
            return Some(self.root_hash);
        }

        let inner_layer = layer - 1;
        self.inner[inner_layer]
            .get(self.get_bucket(inner_layer))
            .map(|node| node.hash)
    }

    // Sets the leaf node to the given block id. Returns the previous block id, if any.
    pub fn set_leaf(&mut self, block_id: &BlockId) -> Option<BlockId> {
        match self.leaves.modify(&self.locator, block_id, false) {
            ModifyStatus::Updated(old_block_id) => {
                self.recalculate(INNER_LAYER_COUNT);
                Some(old_block_id)
            }
            ModifyStatus::Inserted => {
                self.recalculate(INNER_LAYER_COUNT);
                None
            }
            ModifyStatus::Unchanged => None,
        }
    }

    pub fn remove_leaf(&mut self, locator: &Hash) -> Option<BlockId> {
        let block_id = self.leaves.remove(locator)?.block_id;
        let mut start_layer = INNER_LAYER_COUNT;

        if self.leaves.is_empty() {
            for layer in (0..INNER_LAYER_COUNT).rev() {
                let bucket = self.get_bucket(layer);
                let nodes = &mut self.inner[layer];

                nodes.remove(bucket);

                if nodes.is_empty() {
                    start_layer = layer;
                } else {
                    break;
                }
            }
        }

        self.recalculate(start_layer);

        Some(block_id)
    }

    pub fn get_bucket(&self, inner_layer: usize) -> u8 {
        node::get_bucket(&self.locator, inner_layer)
    }

    /// Recalculate layers from start_layer all the way to the root.
    fn recalculate(&mut self, start_layer: usize) {
        for inner_layer in (0..start_layer).rev() {
            let hash = self.compute_hash_for_layer(inner_layer + 1);
            let bucket = self.get_bucket(inner_layer);
            self.inner[inner_layer].insert(bucket, InnerNode::new(hash, Summary::FULL));
        }

        self.root_hash = self.compute_hash_for_layer(0);
    }

    // Assumes layers higher than `layer` have their hashes/BlockVersions already
    // computed/assigned.
    fn compute_hash_for_layer(&self, layer: usize) -> Hash {
        if layer == INNER_LAYER_COUNT {
            self.leaves.hash()
        } else {
            self.inner[layer].hash()
        }
    }
}

use super::node::{
    self, InnerNode, InnerNodeMap, LeafNodeSet, ModifyStatus, NodeState, SingleBlockPresence,
    Summary, EMPTY_INNER_HASH, INNER_LAYER_COUNT,
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
    pub root_hash: Hash,
    pub root_summary: Summary,
    pub inner: Vec<InnerNodeMap>,
    pub leaves: LeafNodeSet,
}

impl Path {
    pub fn new(root_hash: Hash, root_summary: Summary, locator: Hash) -> Self {
        let inner = vec![InnerNodeMap::default(); INNER_LAYER_COUNT];

        Self {
            locator,
            root_hash,
            root_summary,
            inner,
            leaves: LeafNodeSet::default(),
        }
    }

    pub fn get_leaf(&self) -> Option<(BlockId, SingleBlockPresence)> {
        self.leaves
            .get(&self.locator)
            .map(|node| (node.block_id, node.block_presence))
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
    pub fn set_leaf(
        &mut self,
        block_id: &BlockId,
        block_presence: SingleBlockPresence,
    ) -> Option<BlockId> {
        match self.leaves.modify(&self.locator, block_id, block_presence) {
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
        self.recalculate(INNER_LAYER_COUNT);
        Some(block_id)
    }

    pub fn get_bucket(&self, inner_layer: usize) -> u8 {
        node::get_bucket(&self.locator, inner_layer)
    }

    /// Recalculate layers from start_layer all the way to the root.
    fn recalculate(&mut self, start_layer: usize) {
        for inner_layer in (0..start_layer).rev() {
            let bucket = self.get_bucket(inner_layer);

            if let Some(hash) = self.compute_hash_for_layer(inner_layer + 1) {
                let summary = self.compute_summary_for_layer(inner_layer + 1);
                self.inner[inner_layer].insert(bucket, InnerNode::new(hash, summary));
            } else {
                self.inner[inner_layer].remove(bucket);
            }
        }

        self.root_hash = self.compute_hash_for_layer(0).unwrap_or(*EMPTY_INNER_HASH);
        self.root_summary = self.compute_summary_for_layer(0);
        self.root_summary.state = NodeState::Approved;
    }

    // Assumes layers higher than `layer` have their hashes already computed
    fn compute_hash_for_layer(&self, layer: usize) -> Option<Hash> {
        if layer == INNER_LAYER_COUNT {
            if self.leaves.is_empty() {
                None
            } else {
                Some(self.leaves.hash())
            }
        } else if self.inner[layer].is_empty() {
            None
        } else {
            Some(self.inner[layer].hash())
        }
    }

    // Assumes layers higher than `layer` have their summaries already computed
    fn compute_summary_for_layer(&self, layer: usize) -> Summary {
        if layer == INNER_LAYER_COUNT {
            Summary::from_leaves(&self.leaves)
        } else {
            Summary::from_inners(&self.inner[layer])
        }
    }
}

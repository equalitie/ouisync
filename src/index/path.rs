use crate::{
    block::BlockId,
    crypto::Hash,
    index::{
        node::{InnerNode, LeafNode},
        INNER_LAYER_COUNT, MAX_INNER_NODE_CHILD_COUNT,
    },
};
use sha3::{Digest, Sha3_256};

type InnerChildren = [InnerNode; MAX_INNER_NODE_CHILD_COUNT];

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
pub struct Path {
    locator: Hash,
    /// Count of the number of layers found where a locator has a corresponding bucket. Including
    /// the root and leaf layers.  (e.g. 0 -> root wasn't found; 1 -> root was found but no inner
    /// nor leaf layers was; 2 -> root and one inner (possibly leaf if INNER_LAYER_COUNT == 0)
    /// layers were found; ...)
    pub layers_found: usize,
    pub root: Hash,
    pub inner: [InnerChildren; INNER_LAYER_COUNT],
    /// Note: this vector must be sorted to guarantee unique hashing.
    pub leaves: Vec<LeafNode>,
}

impl Path {
    pub fn new(locator: Hash) -> Self {
        let null_hash = Hash::null();

        let inner =
            [[InnerNode { hash: null_hash }; MAX_INNER_NODE_CHILD_COUNT]; INNER_LAYER_COUNT];

        Self {
            locator,
            layers_found: 0,
            root: null_hash,
            inner,
            leaves: Vec::new(),
        }
    }

    pub fn get_leaf(&self) -> Option<BlockId> {
        self.leaves
            .iter()
            .find(|l| l.locator == self.locator)
            .map(|l| l.block_id)
    }

    pub fn has_leaf(&self, block_id: &BlockId) -> bool {
        self.leaves.iter().any(|l| l.block_id == *block_id)
    }

    pub fn total_layer_count() -> usize {
        1 /* root */ + INNER_LAYER_COUNT + 1 /* leaves */
    }

    pub fn hash_at_layer(&self, layer: usize) -> Hash {
        if layer == 0 {
            return self.root;
        }
        let inner_layer = layer - 1;
        self.inner[inner_layer][self.get_bucket(inner_layer)].hash
    }

    // BlockVersion is needed when calculating hashes at the beginning to make this tree unique
    // across all the branches.
    pub fn set_leaf(&mut self, block_id: &BlockId) {
        if self.has_leaf(block_id) {
            return;
        }

        let mut modified = false;

        for leaf in &mut self.leaves {
            if leaf.locator == self.locator {
                modified = true;
                leaf.block_id = *block_id;
                break;
            }
        }

        if !modified {
            // XXX: This can be done better.
            self.leaves.push(LeafNode {
                locator: self.locator,
                block_id: *block_id,
            });
            self.leaves.sort();
        }

        self.recalculate(INNER_LAYER_COUNT);
    }

    pub fn remove_leaf(&mut self, locator: &Hash) {
        let mut changed = false;

        self.leaves = self
            .leaves
            .iter()
            .filter(|l| {
                let keep = l.locator != *locator;
                if !keep {
                    changed = true;
                }
                keep
            })
            .cloned()
            .collect();

        if !changed {
            return;
        }

        if !self.leaves.is_empty() {
            self.recalculate(INNER_LAYER_COUNT);
            return;
        }

        if INNER_LAYER_COUNT > 0 {
            self.remove_from_inner_layer(INNER_LAYER_COUNT - 1);
        } else {
            self.remove_root_layer();
        }
    }

    pub fn get_bucket(&self, inner_layer: usize) -> usize {
        self.locator.as_ref()[inner_layer] as usize
    }

    fn remove_from_inner_layer(&mut self, inner_layer: usize) {
        let null = Hash::null();
        let bucket = self.get_bucket(inner_layer);

        self.inner[inner_layer][bucket] = InnerNode { hash: null };

        let is_empty = self.inner[inner_layer].iter().all(|x| x.hash == null);

        if !is_empty {
            self.recalculate(inner_layer - 1);
            return;
        }

        if inner_layer > 0 {
            self.remove_from_inner_layer(inner_layer - 1);
        } else {
            self.remove_root_layer();
        }
    }

    fn remove_root_layer(&mut self) {
        self.root = Hash::null();
    }

    /// Recalculate layers from start_layer all the way to the root.
    fn recalculate(&mut self, start_layer: usize) {
        for inner_layer in (0..start_layer).rev() {
            let hash = self.compute_hash_for_layer(inner_layer + 1);
            self.inner[inner_layer][self.get_bucket(inner_layer)] = InnerNode { hash };
        }

        self.root = self.compute_hash_for_layer(0);
    }

    // Assumes layers higher than `layer` have their hashes/BlockVersions already
    // computed/assigned.
    fn compute_hash_for_layer(&self, layer: usize) -> Hash {
        if layer == INNER_LAYER_COUNT {
            hash_leafs(&self.leaves)
        } else {
            hash_inner(&self.inner[layer])
        }
    }
}

fn hash_leafs(leaves: &[LeafNode]) -> Hash {
    let mut hash = Sha3_256::new();
    // XXX: Is updating with length enough to prevent attaks?
    hash.update((leaves.len() as u32).to_le_bytes());
    for ref l in leaves {
        hash.update(l.locator);
        hash.update(l.block_id.name);
        hash.update(l.block_id.version);
    }
    hash.finalize().into()
}

fn hash_inner(siblings: &[InnerNode]) -> Hash {
    // XXX: Have some cryptographer check this whether there are no attacks.
    let mut hash = Sha3_256::new();
    for (k, ref s) in siblings.iter().enumerate() {
        if !s.hash.is_null() {
            hash.update((k as u16).to_le_bytes());
            hash.update(s.hash);
        }
    }
    hash.finalize().into()
}

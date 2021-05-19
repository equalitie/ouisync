use crate::{
    block::BlockId,
    crypto::Hash,
    index::{
        node::{InnerNode, LeafNode},
        Crc, LeafData, INNER_LAYER_COUNT, MAX_INNER_NODE_CHILD_COUNT,
    },
};
use crc::{crc32, Hasher32};
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
    pub missing_blocks_crc: Crc,
    pub missing_blocks_count: usize,
    pub inner: Vec<InnerChildren>,
    /// Note: this vector must be sorted to guarantee unique hashing.
    pub leaves: Vec<LeafNode>,
}

impl Path {
    pub fn new(locator: Hash) -> Self {
        let null_hash = Hash::null();

        let inner = vec![[InnerNode::empty(); MAX_INNER_NODE_CHILD_COUNT]; INNER_LAYER_COUNT];

        Self {
            locator,
            layers_found: 0,
            root: null_hash,
            missing_blocks_crc: 0,
            missing_blocks_count: 0,
            inner,
            leaves: Vec::new(),
        }
    }

    pub fn get_leaf(&self) -> Option<BlockId> {
        self.leaves
            .iter()
            .find(|l| l.data.locator == self.locator)
            .map(|l| l.data.block_id)
    }

    pub fn has_leaf(&self, block_id: &BlockId) -> bool {
        self.leaves.iter().any(|l| l.data.block_id == *block_id)
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
    // across all the snapshots.
    pub fn set_leaf(&mut self, block_id: &BlockId) {
        let locator = &self.locator;

        if let Some(node) = self
            .leaves
            .iter_mut()
            .find(|node| &node.data.locator == locator)
        {
            if &node.data.block_id == block_id {
                // no change
                return;
            }

            node.data.block_id = *block_id;
        } else {
            let new_node = LeafNode {
                data: LeafData {
                    locator: self.locator,
                    block_id: *block_id,
                },
                is_complete: true,
                missing_blocks_crc: 0,
                missing_blocks_count: 0,
            };

            let index = self
                .leaves
                .iter()
                .position(|node| node > &new_node)
                .unwrap_or(self.leaves.len());
            self.leaves.insert(index, new_node);
        }

        self.recalculate(INNER_LAYER_COUNT);
    }

    pub fn remove_leaf(&mut self, locator: &Hash) {
        let mut changed = false;

        self.leaves = self
            .leaves
            .iter()
            .filter(|l| {
                let keep = l.data.locator != *locator;
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

        self.inner[inner_layer][bucket] = InnerNode::empty();

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
            let (hash, crc, cnt) = self.compute_hash_for_layer(inner_layer + 1);
            let bucket = self.get_bucket(inner_layer);
            self.inner[inner_layer][bucket] = InnerNode {
                hash,
                is_complete: true,
                missing_blocks_crc: crc,
                missing_blocks_count: cnt,
            };
        }

        let (hash, crc, cnt) = self.compute_hash_for_layer(0);
        self.root = hash;
        self.missing_blocks_crc = crc;
        self.missing_blocks_count = cnt;
    }

    // Assumes layers higher than `layer` have their hashes/BlockVersions already
    // computed/assigned.
    fn compute_hash_for_layer(&self, layer: usize) -> (Hash, Crc, usize) {
        if layer == INNER_LAYER_COUNT {
            let (crc, cnt) = calculate_missing_blocks_crc_from_leaves(&self.leaves);
            (hash_leaves(&self.leaves), crc, cnt)
        } else {
            let (crc, cnt) = calculate_missing_blocks_crc_from_inner(&self.inner[layer]);
            (hash_inner(&self.inner[layer]), crc, cnt)
        }
    }
}

fn hash_leaves(leaves: &[LeafNode]) -> Hash {
    let mut hash = Sha3_256::new();
    // XXX: Is updating with length enough to prevent attaks?
    hash.update((leaves.len() as u32).to_le_bytes());
    for l in leaves {
        hash.update(l.data.locator);
        hash.update(l.data.block_id.name);
        hash.update(l.data.block_id.version);
    }
    hash.finalize().into()
}

fn hash_inner(siblings: &[InnerNode]) -> Hash {
    // XXX: Have some cryptographer check this whether there are no attacks.
    let mut hash = Sha3_256::new();
    for (k, s) in siblings.iter().enumerate() {
        if !s.hash.is_null() {
            hash.update((k as u16).to_le_bytes());
            hash.update(s.hash);
        }
    }
    hash.finalize().into()
}

fn calculate_missing_blocks_crc_from_leaves(leaves: &[LeafNode]) -> (Crc, usize) {
    let mut cnt = 0;

    if leaves.is_empty() {
        return (0, cnt);
    }

    let mut digest = crc32::Digest::new(crc32::IEEE);

    for l in leaves {
        if l.missing_blocks_crc != 0 {
            cnt += 1;
            digest.write(l.missing_blocks_crc.to_le_bytes().as_ref());
        }
    }

    (digest.sum32(), cnt)
}

fn calculate_missing_blocks_crc_from_inner(inner: &[InnerNode]) -> (Crc, usize) {
    let mut cnt = 0;

    if inner.is_empty() {
        return (0, cnt);
    }

    let mut digest = crc32::Digest::new(crc32::IEEE);

    for n in inner {
        if n.missing_blocks_crc != 0 {
            cnt += 1;
            digest.write(n.missing_blocks_crc.to_le_bytes().as_ref());
        }
    }

    (digest.sum32(), cnt)
}

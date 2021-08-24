use super::{get_bucket, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet, INNER_LAYER_COUNT};
use crate::{
    block::BlockId,
    crypto::{Hash, Hashable},
};
use rand::Rng;
use std::{collections::HashMap, mem};

// In-memory snapshot for testing purposes.
pub(crate) struct Snapshot {
    root_hash: Hash,
    inners: [HashMap<BucketPath, InnerNodeMap>; INNER_LAYER_COUNT],
    leaves: HashMap<BucketPath, LeafNodeSet>,
}

impl Snapshot {
    // Generate a random snapshot with the given maximum number of leaf nodes.
    pub fn generate<R: Rng>(rng: &mut R, leaf_count: usize) -> Self {
        let leaves = (0..leaf_count)
            .map(|_| {
                let locator = rng.gen::<u64>().hash();
                let block_id = rng.gen();
                LeafNode::present(locator, block_id)
            })
            .collect();

        Self::from_leaves(leaves)
    }

    pub fn from_leaves(leaves: Vec<LeafNode>) -> Self {
        let leaves = leaves
            .into_iter()
            .fold(HashMap::<_, LeafNodeSet>::new(), |mut map, leaf| {
                map.entry(BucketPath::new(leaf.locator(), INNER_LAYER_COUNT - 1))
                    .or_default()
                    .modify(leaf.locator(), &leaf.block_id, true);
                map
            });

        let mut inners: [HashMap<_, InnerNodeMap>; INNER_LAYER_COUNT] = Default::default();

        for (path, set) in &leaves {
            add_inner_node(
                INNER_LAYER_COUNT - 1,
                &mut inners[INNER_LAYER_COUNT - 1],
                path,
                set.hash(),
            );
        }

        for layer in (0..INNER_LAYER_COUNT - 1).rev() {
            let (lo, hi) = inners.split_at_mut(layer + 1);

            for (path, map) in &hi[0] {
                add_inner_node(layer, lo.last_mut().unwrap(), path, map.hash());
            }
        }

        let root_hash = inners[0]
            .get(&BucketPath::default())
            .unwrap_or(&InnerNodeMap::default())
            .hash();

        Self {
            root_hash,
            inners,
            leaves,
        }
    }

    pub fn root_hash(&self) -> &Hash {
        &self.root_hash
    }

    pub fn leaf_sets(&self) -> impl Iterator<Item = (&Hash, &LeafNodeSet)> {
        self.leaves.iter().map(move |(path, nodes)| {
            let parent_hash = self.parent_hash(INNER_LAYER_COUNT, path);
            (parent_hash, nodes)
        })
    }

    pub fn leaf_count(&self) -> usize {
        self.leaves.values().map(|nodes| nodes.len()).sum()
    }

    pub fn inner_layers(&self) -> impl Iterator<Item = InnerLayer> {
        (0..self.inners.len()).map(move |inner_layer| InnerLayer(self, inner_layer))
    }

    pub fn block_ids(&self) -> impl Iterator<Item = &BlockId> {
        self.leaves
            .values()
            .flat_map(|nodes| nodes.iter().map(|node| &node.block_id))
    }

    // Returns the parent hash of inner nodes at `inner_layer` with the specified bucket path.
    fn parent_hash(&self, inner_layer: usize, path: &BucketPath) -> &Hash {
        if inner_layer == 0 {
            &self.root_hash
        } else {
            let (bucket, parent_path) = path.pop(inner_layer - 1);
            &self.inners[inner_layer - 1]
                .get(&parent_path)
                .unwrap()
                .get(bucket)
                .unwrap()
                .hash
        }
    }
}

pub(crate) struct InnerLayer<'a>(&'a Snapshot, usize);

impl<'a> InnerLayer<'a> {
    pub fn inner_maps(&self) -> impl Iterator<Item = (&Hash, &InnerNodeMap)> {
        self.0.inners[self.1].iter().map(move |(path, nodes)| {
            let parent_hash = self.0.parent_hash(self.1, path);
            (parent_hash, nodes)
        })
    }

    pub fn number(&self) -> usize {
        self.1
    }
}

fn add_inner_node(
    inner_layer: usize,
    maps: &mut HashMap<BucketPath, InnerNodeMap>,
    path: &BucketPath,
    hash: Hash,
) {
    let (bucket, parent_path) = path.pop(inner_layer);
    maps.entry(parent_path)
        .or_default()
        .insert(bucket, InnerNode::new(hash));
}

#[derive(Default, Clone, Copy, Eq, PartialEq, Hash, Debug)]
struct BucketPath([u8; INNER_LAYER_COUNT]);

impl BucketPath {
    fn new(locator: &Hash, inner_layer: usize) -> Self {
        let mut path = Self(Default::default());
        for (layer, bucket) in path.0.iter_mut().enumerate().take(inner_layer + 1) {
            *bucket = get_bucket(locator, layer)
        }
        path
    }

    fn pop(&self, inner_layer: usize) -> (u8, Self) {
        let mut popped = *self;
        let bucket = mem::replace(&mut popped.0[inner_layer], 0);
        (bucket, popped)
    }
}

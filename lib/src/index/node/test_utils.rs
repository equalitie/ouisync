use super::{
    super::{Index, Proof},
    get_bucket, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet, Summary, INNER_LAYER_COUNT,
};
use crate::{
    block::{BlockData, BlockId, BlockNonce, BLOCK_SIZE},
    crypto::{
        sign::{Keypair, PublicKey},
        Hash, Hashable,
    },
    store,
    version_vector::VersionVector,
};
use rand::{
    distributions::{Distribution, Standard},
    Rng,
};
use std::{collections::HashMap, mem};

// In-memory snapshot for testing purposes.
pub(crate) struct Snapshot {
    root_hash: Hash,
    inners: [HashMap<BucketPath, InnerNodeMap>; INNER_LAYER_COUNT],
    leaves: HashMap<BucketPath, LeafNodeSet>,
    blocks: HashMap<BlockId, Block>,
}

impl Snapshot {
    // Generate a random snapshot with the given number of blocks.
    pub fn generate<R: Rng>(rng: &mut R, block_count: usize) -> Self {
        Self::new(rng.sample_iter(Standard).take(block_count))
    }

    // Create snapshot given an iterator of blocks where each block is associated to its encoded
    // locator.
    pub fn new(blocks_and_locators: impl IntoIterator<Item = (Block, Hash)>) -> Self {
        let mut blocks = HashMap::new();
        let mut leaves = HashMap::new();

        for (block, locator) in blocks_and_locators {
            let id = block.data.id;
            blocks.insert(id, block);

            let node = LeafNode::present(locator, id);
            leaves
                .entry(BucketPath::new(node.locator(), INNER_LAYER_COUNT - 1))
                .or_insert_with(LeafNodeSet::default)
                .modify(node.locator(), &node.block_id, true);
        }

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
            blocks,
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

    pub fn blocks(&self) -> &HashMap<BlockId, Block> {
        &self.blocks
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
}

#[derive(Clone)]
pub(crate) struct Block {
    pub data: BlockData,
    pub nonce: BlockNonce,
}

impl Distribution<Block> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> Block {
        let mut content = vec![0; BLOCK_SIZE];
        rng.fill(&mut content[..]);

        let nonce = rng.gen();

        Block {
            data: BlockData::from(content.into_boxed_slice()),
            nonce,
        }
    }
}

// Receive all nodes in `snapshot` into `index`.
pub(crate) async fn receive_nodes(
    index: &Index,
    write_keys: &Keypair,
    branch_id: PublicKey,
    version_vector: VersionVector,
    snapshot: &Snapshot,
) {
    let _enable = index.enable_receive_filter();

    let proof = Proof::new(branch_id, version_vector, *snapshot.root_hash(), write_keys);
    index
        .receive_root_node(proof.into(), Summary::INCOMPLETE)
        .await
        .unwrap();

    for layer in snapshot.inner_layers() {
        for (_, nodes) in layer.inner_maps() {
            index
                .receive_inner_nodes(nodes.clone().into())
                .await
                .unwrap();
        }
    }

    for (_, nodes) in snapshot.leaf_sets() {
        index
            .receive_leaf_nodes(nodes.clone().into())
            .await
            .unwrap();
    }
}

pub(crate) async fn receive_blocks(index: &Index, snapshot: &Snapshot) {
    for block in snapshot.blocks().values() {
        store::write_received_block(index, &block.data, &block.nonce)
            .await
            .unwrap();
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
        .insert(bucket, InnerNode::new(hash, Summary::INCOMPLETE));
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

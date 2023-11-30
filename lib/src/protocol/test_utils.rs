use super::{proof::Proof, MultiBlockPresence, NodeState, SingleBlockPresence, Summary};
use crate::{
    block_tracker::OfferState,
    collections::HashMap,
    crypto::{
        sign::{Keypair, PublicKey},
        Hash, Hashable,
    },
    protocol::{
        get_bucket, Block, BlockId, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet,
        INNER_LAYER_COUNT,
    },
    repository::Vault,
    store::ReceiveFilter,
    version_vector::VersionVector,
};
use rand::{distributions::Standard, Rng};
use std::mem;

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
    pub fn new(locators_and_blocks: impl IntoIterator<Item = (Hash, Block)>) -> Self {
        let mut blocks = HashMap::default();
        let mut leaves = HashMap::default();

        for (locator, block) in locators_and_blocks {
            let id = block.id;
            blocks.insert(id, block);

            let node = LeafNode::present(locator, id);
            leaves
                .entry(BucketPath::new(&node.locator, INNER_LAYER_COUNT - 1))
                .or_insert_with(LeafNodeSet::default)
                .insert(node.locator, node.block_id, SingleBlockPresence::Present);
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

// Receive all nodes in `snapshot` into `index`.
pub(crate) async fn receive_nodes(
    vault: &Vault,
    write_keys: &Keypair,
    branch_id: PublicKey,
    version_vector: VersionVector,
    receive_filter: &ReceiveFilter,
    snapshot: &Snapshot,
) {
    let proof = Proof::new(branch_id, version_vector, *snapshot.root_hash(), write_keys);
    vault
        .receive_root_node(proof.into(), MultiBlockPresence::Full)
        .await
        .unwrap();

    for layer in snapshot.inner_layers() {
        for (_, nodes) in layer.inner_maps() {
            vault
                .receive_inner_nodes(nodes.clone().into(), receive_filter, None)
                .await
                .unwrap();
        }
    }

    for (_, nodes) in snapshot.leaf_sets() {
        vault
            .receive_leaf_nodes(nodes.clone().into(), None)
            .await
            .unwrap();
    }
}

pub(crate) async fn receive_blocks(repo: &Vault, snapshot: &Snapshot) {
    let client = repo.block_tracker.client();
    let offers = client.offers();

    for block in snapshot.blocks().values() {
        repo.block_tracker.require(block.id);
        client.register(block.id, OfferState::Approved);
        let promise = offers.try_next().unwrap().accept().unwrap();

        repo.receive_block(block, Some(promise)).await.unwrap();
    }
}

fn add_inner_node(
    inner_layer: usize,
    maps: &mut HashMap<BucketPath, InnerNodeMap>,
    path: &BucketPath,
    hash: Hash,
) {
    let (bucket, parent_path) = path.pop(inner_layer);
    maps.entry(parent_path).or_default().insert(
        bucket,
        InnerNode::new(
            hash,
            Summary {
                state: NodeState::Complete,
                block_presence: MultiBlockPresence::Full,
            },
        ),
    );
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

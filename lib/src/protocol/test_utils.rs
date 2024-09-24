use super::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary};
use crate::{
    collections::HashMap,
    crypto::{Hash, Hashable},
    protocol::{
        get_bucket, Block, BlockId, InnerNode, InnerNodes, LeafNode, LeafNodes, INNER_LAYER_COUNT,
    },
};
use rand::{distributions::Standard, Rng};
use std::{borrow::Cow, fmt, mem};

// In-memory snapshot for testing purposes.
#[derive(Clone)]
pub(crate) struct Snapshot {
    root_hash: Hash,
    root_block_presence: MultiBlockPresence,
    inners: [HashMap<BucketPath, InnerNodes>; INNER_LAYER_COUNT],
    leaves: HashMap<BucketPath, LeafNodes>,
    blocks: HashMap<BlockId, Block>,
}

impl Snapshot {
    // Generate a random snapshot with the given number of blocks.
    pub fn generate<R: Rng>(rng: &mut R, block_count: usize) -> Self {
        Self::from_present_blocks(rng.sample_iter(Standard).take(block_count))
    }

    pub fn from_present_blocks(
        locators_and_blocks: impl IntoIterator<Item = (Hash, Block)>,
    ) -> Self {
        Self::from_blocks(
            locators_and_blocks
                .into_iter()
                .map(|(locator, block)| (locator, BlockState::Present(block))),
        )
    }

    // Create snapshot given an iterator of blocks where each block is associated to its encoded
    // locator.
    pub fn from_blocks(locators_and_blocks: impl IntoIterator<Item = (Hash, BlockState)>) -> Self {
        let mut blocks = HashMap::default();
        let mut leaves = HashMap::default();

        for (locator, block) in locators_and_blocks {
            let block_id = *block.id();
            let block_presence = block.presence();

            match block {
                BlockState::Present(block) => {
                    blocks.insert(block_id, block);
                }
                BlockState::Missing(_) => (),
            }

            leaves
                .entry(BucketPath::new(&locator, INNER_LAYER_COUNT - 1))
                .or_insert_with(LeafNodes::default)
                .insert(locator, block_id, block_presence);
        }

        let mut inners: [HashMap<_, InnerNodes>; INNER_LAYER_COUNT] = Default::default();

        for (path, nodes) in &leaves {
            add_inner_node(
                INNER_LAYER_COUNT - 1,
                &mut inners[INNER_LAYER_COUNT - 1],
                path,
                nodes.hash(),
                Summary::from_leaves(nodes).block_presence,
            );
        }

        for layer in (0..INNER_LAYER_COUNT - 1).rev() {
            let (lo, hi) = inners.split_at_mut(layer + 1);

            for (path, nodes) in &hi[0] {
                add_inner_node(
                    layer,
                    lo.last_mut().unwrap(),
                    path,
                    nodes.hash(),
                    Summary::from_inners(nodes).block_presence,
                );
            }
        }

        let nodes = inners[0]
            .get(&BucketPath::default())
            .map(Cow::Borrowed)
            .unwrap_or(Cow::Owned(InnerNodes::default()));
        let root_hash = nodes.hash();
        let root_block_presence = Summary::from_inners(&nodes).block_presence;

        Self {
            root_hash,
            root_block_presence,
            inners,
            leaves,
            blocks,
        }
    }

    pub fn root_hash(&self) -> &Hash {
        &self.root_hash
    }

    #[expect(dead_code)]
    pub fn root_block_presence(&self) -> &MultiBlockPresence {
        &self.root_block_presence
    }

    pub fn leaf_sets(&self) -> impl Iterator<Item = (&Hash, &LeafNodes)> {
        self.leaves.iter().map(move |(path, nodes)| {
            let parent_hash = self.parent_hash(INNER_LAYER_COUNT, path);
            (parent_hash, nodes)
        })
    }

    pub fn leaf_nodes(&self) -> impl Iterator<Item = &LeafNode> {
        self.leaves.iter().flat_map(|(_, nodes)| nodes)
    }

    pub fn locators_and_blocks(&self) -> impl Iterator<Item = (&Hash, &Block)> {
        self.leaf_nodes().filter_map(|node| {
            self.blocks
                .get(&node.block_id)
                .map(|block| (&node.locator, block))
        })
    }

    pub fn leaf_count(&self) -> usize {
        self.leaves.values().map(|nodes| nodes.len()).sum()
    }

    pub fn inner_layers(&self) -> impl Iterator<Item = InnerLayer> {
        (0..self.inners.len()).map(move |inner_layer| InnerLayer(self, inner_layer))
    }

    pub fn inner_nodes(&self) -> impl Iterator<Item = &InnerNode> {
        self.inners
            .iter()
            .flat_map(|layer| layer.values())
            .flatten()
            .map(|(_, node)| node)
    }

    pub fn inner_count(&self) -> usize {
        self.inners
            .iter()
            .flat_map(|layer| layer.values())
            .map(|nodes| nodes.len())
            .sum()
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

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("root_hash", &self.root_hash)
            .field("num_blocks", &self.blocks.len())
            .finish_non_exhaustive()
    }
}

pub(crate) struct InnerLayer<'a>(&'a Snapshot, usize);

impl<'a> InnerLayer<'a> {
    pub fn inner_maps(self) -> impl Iterator<Item = (&'a Hash, &'a InnerNodes)> {
        self.0.inners[self.1].iter().map(move |(path, nodes)| {
            let parent_hash = self.0.parent_hash(self.1, path);
            (parent_hash, nodes)
        })
    }
}

pub(crate) enum BlockState {
    Present(Block),
    #[expect(dead_code)]
    Missing(BlockId),
}

impl BlockState {
    pub fn id(&self) -> &BlockId {
        match self {
            Self::Present(block) => &block.id,
            Self::Missing(block_id) => block_id,
        }
    }

    pub fn presence(&self) -> SingleBlockPresence {
        match self {
            Self::Present(_) => SingleBlockPresence::Present,
            Self::Missing(_) => SingleBlockPresence::Missing,
        }
    }
}

fn add_inner_node(
    inner_layer: usize,
    maps: &mut HashMap<BucketPath, InnerNodes>,
    path: &BucketPath,
    hash: Hash,
    block_presence: MultiBlockPresence,
) {
    let (bucket, parent_path) = path.pop(inner_layer);
    maps.entry(parent_path).or_default().insert(
        bucket,
        InnerNode::new(
            hash,
            Summary {
                state: NodeState::Complete,
                block_presence,
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

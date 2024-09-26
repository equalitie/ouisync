use super::{MultiBlockPresence, NodeState, SingleBlockPresence, Summary, EMPTY_INNER_HASH};
use crate::{
    crypto::{Hash, Hashable},
    protocol::{Block, BlockId, InnerNode, InnerNodes, LeafNode, LeafNodes, INNER_LAYER_COUNT},
};
use rand::{distributions::Standard, Rng};
use std::{
    borrow::Cow,
    collections::{btree_map::Entry, BTreeMap, VecDeque},
    fmt, mem,
};

// In-memory snapshot for testing purposes.
#[derive(Clone)]
pub(crate) struct Snapshot {
    root_hash: Hash,
    root_summary: Summary,
    // Using BTreeMap instead of HashMap for deterministic iteration order for repeatable tests.
    inners: BTreeMap<Hash, InnerNodes>,
    leaves: BTreeMap<Hash, LeafNodes>,
    blocks: BTreeMap<BlockId, Block>,
}

impl Snapshot {
    // Generate a random snapshot with the given number of blocks.
    pub fn generate<R: Rng>(rng: &mut R, block_count: usize) -> Self {
        Self::from_present_blocks(rng.sample_iter(Standard).take(block_count))
    }

    // Convenience alternative to `from_blocks` when all blocks are present.
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
        let mut blocks = BTreeMap::default();
        let mut leaves = BTreeMap::default();
        let mut inners = Vec::new();

        for (locator, block) in locators_and_blocks {
            let block_id = *block.id();
            let block_presence = match block {
                BlockState::Present(block) => {
                    blocks.insert(block_id, block);
                    SingleBlockPresence::Present
                }
                BlockState::Missing(_) => SingleBlockPresence::Missing,
            };

            let path = BucketPath::leaf(&locator);

            leaves
                .entry(path)
                .or_insert_with(LeafNodes::default)
                .insert(locator, block_id, block_presence);
        }

        let mut layer = BTreeMap::default();

        for (&path, child_nodes) in &leaves {
            let parent_node = InnerNode::new(child_nodes.hash(), Summary::from_leaves(child_nodes));
            let (bucket, path) = path.pop(INNER_LAYER_COUNT - 1);

            layer
                .entry(path)
                .or_insert_with(InnerNodes::default)
                .insert(bucket, parent_node);
        }

        inners.push(layer);

        for layer_index in (0..INNER_LAYER_COUNT - 1).rev() {
            let mut layer = BTreeMap::default();

            for (&path, child_nodes) in inners.last().unwrap() {
                let parent_node =
                    InnerNode::new(child_nodes.hash(), Summary::from_inners(child_nodes));
                let (bucket, path) = path.pop(layer_index);

                layer
                    .entry(path)
                    .or_insert_with(InnerNodes::default)
                    .insert(bucket, parent_node);
            }

            inners.push(layer);
        }

        let nodes = inners
            .last()
            .and_then(|layer| layer.values().next())
            .map(Cow::Borrowed)
            .unwrap_or(Cow::Owned(InnerNodes::default()));
        let root_hash = nodes.hash();
        let root_summary = Summary::from_inners(&nodes);

        Self {
            root_hash,
            root_summary,
            inners: inners
                .into_iter()
                .flat_map(|layer| layer.into_values())
                .map(|nodes| (nodes.hash(), nodes))
                .collect(),
            leaves: leaves
                .into_values()
                .map(|nodes| (nodes.hash(), nodes))
                .collect(),
            blocks,
        }
    }

    pub fn root_hash(&self) -> &Hash {
        &self.root_hash
    }

    pub fn root_summary(&self) -> &Summary {
        &self.root_summary
    }

    pub fn leaf_sets(&self) -> impl Iterator<Item = (&Hash, &LeafNodes)> {
        self.leaves.iter()
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

    pub fn get_leaf_set(&self, parent_hash: &Hash) -> Option<&LeafNodes> {
        self.leaves.get(parent_hash)
    }

    // Iterates the inner sets in topological order (parents before children)
    pub fn inner_sets(&self) -> impl Iterator<Item = (&Hash, &InnerNodes)> {
        InnerSets {
            inners: &self.inners,
            queue: VecDeque::from([&self.root_hash]),
        }
    }

    pub fn inner_nodes(&self) -> impl Iterator<Item = &InnerNode> {
        self.inner_sets()
            .flat_map(|(_, nodes)| nodes)
            .map(|(_, node)| node)
    }

    pub fn inner_count(&self) -> usize {
        self.inners.values().map(|nodes| nodes.len()).sum()
    }

    pub fn get_inner_set(&self, parent_hash: &Hash) -> Option<&InnerNodes> {
        self.inners.get(parent_hash)
    }

    pub fn blocks(&self) -> &BTreeMap<BlockId, Block> {
        &self.blocks
    }

    pub fn insert_root(&mut self, hash: Hash, block_presence: MultiBlockPresence) -> bool {
        if self.root_hash == hash {
            match self.root_summary.state {
                NodeState::Incomplete => true,
                NodeState::Complete | NodeState::Approved => self
                    .root_summary
                    .block_presence
                    .is_outdated(&block_presence),
                NodeState::Rejected => unimplemented!(),
            }
        } else {
            self.root_hash = hash;
            self.root_summary = if hash == *EMPTY_INNER_HASH {
                Summary {
                    state: NodeState::Complete,
                    block_presence: MultiBlockPresence::None,
                }
            } else {
                Summary::INCOMPLETE
            };

            self.inners.clear();
            self.leaves.clear();
            self.blocks.clear();

            true
        }
    }

    pub fn insert_inners(&mut self, nodes: InnerNodes) -> InnerNodes {
        match self.inners.entry(nodes.hash()) {
            Entry::Occupied(entry) => entry
                .get()
                .iter()
                .zip(nodes)
                .filter(|((_, old), (_, new))| old.summary.is_outdated(&new.summary))
                .map(|(_, new)| new)
                .collect(),
            Entry::Vacant(entry) => {
                entry.insert(nodes.clone().into_incomplete());
                nodes
            }
        }
    }

    pub fn insert_leaves(&mut self, nodes: LeafNodes) -> LeafNodes {
        let (nodes, update) = match self.leaves.entry(nodes.hash()) {
            Entry::Occupied(entry) => (
                entry
                    .get()
                    .iter()
                    .zip(nodes)
                    .filter(
                        |(old, new)| match (old.block_presence, new.block_presence) {
                            (SingleBlockPresence::Missing, SingleBlockPresence::Present) => true,
                            (SingleBlockPresence::Present, SingleBlockPresence::Present)
                            | (SingleBlockPresence::Present, SingleBlockPresence::Missing)
                            | (SingleBlockPresence::Missing, SingleBlockPresence::Missing) => false,
                            (SingleBlockPresence::Expired, _)
                            | (_, SingleBlockPresence::Expired) => unimplemented!(),
                        },
                    )
                    .map(|(_, new)| new)
                    .collect(),
                false,
            ),
            Entry::Vacant(entry) => {
                entry.insert(nodes.clone().into_missing());
                (nodes, true)
            }
        };

        if update {
            self.update_root_summary();
        }

        nodes
    }

    pub fn insert_block(&mut self, block: Block) -> bool {
        if self.blocks.insert(block.id, block).is_none() {
            self.update_root_summary();
            true
        } else {
            false
        }
    }

    fn update_root_summary(&mut self) {
        self.root_summary = self.update_summary(self.root_hash);
    }

    fn update_summary(&mut self, hash: Hash) -> Summary {
        // XXX: This is not very efficient but probably fine for tests.

        // Dancing around the borrow checker ...
        let inner_inputs = self.inners.get(&hash).map(|nodes| {
            nodes
                .iter()
                .filter(|(_, node)| {
                    node.summary.state == NodeState::Incomplete
                        || node.summary.block_presence != MultiBlockPresence::Full
                })
                .map(|(bucket, node)| (bucket, node.hash))
                .collect::<Vec<_>>()
        });

        if let Some(inputs) = inner_inputs {
            let outputs: Vec<_> = inputs
                .into_iter()
                .map(|(bucket, hash)| (bucket, self.update_summary(hash)))
                .collect();

            let nodes = self.inners.get_mut(&hash).unwrap();

            for (bucket, summary) in outputs {
                nodes.get_mut(bucket).unwrap().summary = summary
            }

            Summary::from_inners(nodes)
        } else if let Some(nodes) = self.leaves.get_mut(&hash) {
            for node in &mut *nodes {
                match node.block_presence {
                    SingleBlockPresence::Present => continue,
                    SingleBlockPresence::Missing => {
                        if self.blocks.contains_key(&node.block_id) {
                            node.block_presence = SingleBlockPresence::Present;
                        }
                    }
                    SingleBlockPresence::Expired => unimplemented!(),
                }
            }

            Summary::from_leaves(nodes)
        } else {
            Summary::INCOMPLETE
        }
    }
}

impl Default for Snapshot {
    fn default() -> Self {
        Self::from_blocks([])
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

// Iterator that yields the inner sets in topological order.
struct InnerSets<'a> {
    inners: &'a BTreeMap<Hash, InnerNodes>,
    queue: VecDeque<&'a Hash>,
}

impl<'a> Iterator for InnerSets<'a> {
    type Item = (&'a Hash, &'a InnerNodes);

    fn next(&mut self) -> Option<Self::Item> {
        let parent_hash = self.queue.pop_front()?;
        let nodes = self.inners.get(parent_hash)?;

        self.queue.extend(nodes.iter().map(|(_, node)| &node.hash));

        Some((parent_hash, nodes))
    }
}

#[track_caller]
pub(crate) fn assert_snapshots_equal(lhs: &Snapshot, rhs: &Snapshot) {
    assert_eq!(
        (lhs.root_hash(), lhs.root_summary()),
        (rhs.root_hash(), rhs.root_summary()),
        "root node mismatch"
    );

    assert_eq!(
        lhs.inner_count(),
        rhs.inner_count(),
        "inner node count mismatch"
    );

    for (lhs, rhs) in lhs.inner_nodes().zip(rhs.inner_nodes()) {
        assert_eq!(lhs, rhs, "inner node mismatch");
    }

    assert_eq!(
        lhs.leaf_count(),
        rhs.leaf_count(),
        "leaf node count mismatch"
    );

    for (lhs, rhs) in lhs.leaf_nodes().zip(rhs.leaf_nodes()) {
        assert_eq!(lhs, rhs, "leaf node mismatch");
    }

    assert_eq!(
        lhs.blocks().len(),
        rhs.blocks().len(),
        "present block count mismatch"
    );

    for (lhs, rhs) in lhs.blocks().keys().zip(rhs.blocks().keys()) {
        assert_eq!(lhs, rhs, "block mismatch");
    }
}

#[derive(Debug)]
pub(crate) enum BlockState {
    Present(Block),
    Missing(BlockId),
}

impl BlockState {
    pub fn id(&self) -> &BlockId {
        match self {
            Self::Present(block) => &block.id,
            Self::Missing(block_id) => block_id,
        }
    }
}

#[derive(Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct BucketPath([u8; INNER_LAYER_COUNT]);

impl BucketPath {
    fn leaf(locator: &Hash) -> Self {
        let mut path = [0; INNER_LAYER_COUNT];
        path.copy_from_slice(&locator.as_ref()[..INNER_LAYER_COUNT]);

        Self(path)
    }

    fn pop(mut self, layer: usize) -> (u8, Self) {
        let bucket = mem::replace(&mut self.0[layer], 0);
        (bucket, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{collections::HashSet, protocol::EMPTY_INNER_HASH};
    use rand::{rngs::StdRng, SeedableRng};

    #[test]
    fn empty_snapshot() {
        let s = Snapshot::default();

        assert_eq!(*s.root_hash(), *EMPTY_INNER_HASH);
        assert_eq!(s.root_summary().state, NodeState::Complete);
        assert_eq!(s.root_summary().block_presence, MultiBlockPresence::None);
        assert_eq!(s.inner_count(), 0);
        assert_eq!(s.leaf_count(), 0);
        assert!(s.blocks().is_empty());
    }

    #[test]
    fn full_snapshot() {
        case(rand::random(), 8);

        fn case(seed: u64, max_blocks: usize) {
            println!("seed = {seed}, max_blocks = {max_blocks}");

            let mut rng = StdRng::seed_from_u64(seed);
            let num_blocks = rng.gen_range(1..=max_blocks);
            let s = Snapshot::generate(&mut rng, num_blocks);

            assert_ne!(*s.root_hash(), *EMPTY_INNER_HASH);
            assert_eq!(s.root_summary().state, NodeState::Complete);
            assert_eq!(s.root_summary().block_presence, MultiBlockPresence::Full);
            assert_eq!(s.leaf_count(), num_blocks);
            assert_eq!(s.blocks().len(), num_blocks);

            for node in s.inner_nodes() {
                assert_eq!(node.summary.state, NodeState::Complete);
                assert_eq!(node.summary.block_presence, MultiBlockPresence::Full);
            }

            for node in s.leaf_nodes() {
                assert_eq!(node.block_presence, SingleBlockPresence::Present);
            }

            // Verify that traversing the whole tree yields all the block ids.
            let mut hashes = vec![s.root_hash()];
            let mut expected_block_ids = HashSet::default();

            while let Some(hash) = hashes.pop() {
                if let Some(nodes) = s.get_inner_set(hash) {
                    assert_eq!(nodes.hash(), *hash);
                    hashes.extend(nodes.iter().map(|(_, node)| &node.hash));
                } else if let Some(nodes) = s.get_leaf_set(hash) {
                    assert_eq!(nodes.hash(), *hash);
                    expected_block_ids.extend(nodes.iter().map(|node| &node.block_id));
                } else {
                    panic!("nodes with parent hash {hash:?} not found");
                }
            }

            let actual_block_ids: HashSet<_> = s.blocks().keys().collect();
            assert_eq!(actual_block_ids, expected_block_ids);
        }
    }

    #[test]
    fn snapshot_inner_sets_topological_order() {
        case(rand::random(), 8);

        fn case(seed: u64, max_blocks: usize) {
            println!("seed = {seed}, max_blocks = {max_blocks}");

            let mut rng = StdRng::seed_from_u64(seed);
            let num_blocks = rng.gen_range(1..=max_blocks);
            let s = Snapshot::generate(&mut rng, num_blocks);

            let mut visited = HashSet::from([s.root_hash()]);

            for (parent_hash, nodes) in s.inner_sets() {
                assert!(visited.contains(parent_hash));
                visited.extend(nodes.iter().map(|(_, node)| &node.hash));
            }
        }
    }

    #[test]
    fn snapshot_sync() {
        case(rand::random(), 8);

        fn case(seed: u64, max_blocks: usize) {
            println!("seed = {seed}, max_blocks = {max_blocks}");

            let mut rng = StdRng::seed_from_u64(seed);
            let block_count = rng.gen_range(0..=max_blocks);

            let src = Snapshot::generate(&mut rng, block_count);
            let mut dst = Snapshot::default();

            let result = dst.insert_root(*src.root_hash(), src.root_summary().block_presence);

            let expected_initial_root_state = if block_count == 0 {
                NodeState::Complete
            } else {
                NodeState::Incomplete
            };

            assert_eq!(result, block_count != 0);
            assert_eq!(
                *dst.root_summary(),
                Summary {
                    state: expected_initial_root_state,
                    block_presence: MultiBlockPresence::None
                }
            );

            for (_, nodes) in src.inner_sets() {
                assert_eq!(dst.insert_inners(nodes.clone()), *nodes);
            }

            assert_eq!(dst.root_summary().state, expected_initial_root_state);
            assert_eq!(dst.root_summary().block_presence, MultiBlockPresence::None);

            for node in dst.inner_nodes() {
                assert_eq!(node.summary.state, expected_initial_root_state);
                assert_eq!(node.summary.block_presence, MultiBlockPresence::None);
            }

            for (_, nodes) in src.leaf_sets() {
                assert_eq!(dst.insert_leaves(nodes.clone()), *nodes);
            }

            assert_eq!(dst.root_summary().state, NodeState::Complete);
            assert_eq!(dst.root_summary().block_presence, MultiBlockPresence::None);

            for node in dst.inner_nodes() {
                assert_eq!(node.summary.state, NodeState::Complete);
                assert_eq!(node.summary.block_presence, MultiBlockPresence::None);
            }

            for node in dst.leaf_nodes() {
                assert_eq!(node.block_presence, SingleBlockPresence::Missing);
            }

            for block in src.blocks().values() {
                assert!(dst.insert_block(block.clone()));
            }

            assert_snapshots_equal(&src, &dst);
        }
    }
}

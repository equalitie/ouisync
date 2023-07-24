use crate::{
    crypto::Hash,
    deadlock::BlockingMutex,
    protocol::{InnerNodeMap, LeafNodeSet},
};
use lru::LruCache;
use std::num::NonZeroUsize;

/// Cache for index nodes
pub(super) struct Cache {
    inners: BlockingMutex<LruCache<Hash, InnerNodeMap>>,
    leaves: BlockingMutex<LruCache<Hash, LeafNodeSet>>,
}

impl Cache {
    pub fn new() -> Self {
        const ERROR: &str = "cache capacity must be non-zero";

        Self {
            inners: BlockingMutex::new(LruCache::new(
                NonZeroUsize::new(INNERS_CAPACITY).expect(ERROR),
            )),
            leaves: BlockingMutex::new(LruCache::new(
                NonZeroUsize::new(LEAVES_CAPACITY).expect(ERROR),
            )),
        }
    }

    pub fn get_inners(&self, parent_hash: &Hash) -> Option<InnerNodeMap> {
        self.inners.lock().unwrap().get(parent_hash).cloned()
    }

    pub fn put_inners(&self, parent_hash: Hash, nodes: InnerNodeMap) {
        self.inners.lock().unwrap().put(parent_hash, nodes);
    }

    pub fn get_leaves(&self, parent_hash: &Hash) -> Option<LeafNodeSet> {
        self.leaves.lock().unwrap().get(parent_hash).cloned()
    }

    pub fn put_leaves(&self, parent_hash: Hash, nodes: LeafNodeSet) {
        self.leaves.lock().unwrap().put(parent_hash, nodes);
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

// Max number of leaf node sets in the cache.
const LEAVES_CAPACITY: usize = 1024;

// Max number of inner node maps in the cache. Assuming nodes are uniformly distributed, there
// should be roughly twice as many inner nodes than leaf nodes (for number of leaf nodes < 65536).
const INNERS_CAPACITY: usize = 2 * LEAVES_CAPACITY;

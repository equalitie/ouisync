use crate::{crypto::Hash, deadlock::BlockingMutex, protocol::InnerNodeMap};
use lru::LruCache;
use std::num::NonZeroUsize;

/// Cache for index nodes
pub(super) struct Cache {
    inners: BlockingMutex<LruCache<Hash, InnerNodeMap>>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            inners: BlockingMutex::new(LruCache::new(
                NonZeroUsize::new(INNER_CAPACITY).expect("cache capacity must be non-zero"),
            )),
        }
    }

    pub fn get_inners(&self, parent_hash: &Hash) -> Option<InnerNodeMap> {
        self.inners.lock().unwrap().get(parent_hash).cloned()
    }

    pub fn put_inners(&self, parent_hash: Hash, nodes: InnerNodeMap) {
        self.inners.lock().unwrap().put(parent_hash, nodes);
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

// Max number of leaf node sets in the cache. Corresponds to 1GB of data.
const LEAF_CAPACITY: usize = 128;

// Max number of inner node maps in the cache. Assuming nodes are uniformly distributed, there
// should be roughly twice as many inner nodes than leaf nodes (for number of leaf nodes < 65536).
const INNER_CAPACITY: usize = 2 * LEAF_CAPACITY;

use crate::{
    collections::HashMap,
    crypto::{sign::PublicKey, Hash},
    deadlock::BlockingMutex,
    protocol::{InnerNodeMap, LeafNodeSet, RootNode},
};
use lru::LruCache;
use std::{num::NonZeroUsize, sync::Arc};

/// Cache for index nodes
pub(super) struct Cache {
    roots: BlockingMutex<HashMap<PublicKey, RootNode>>,
    inners: BlockingMutex<LruCache<Hash, InnerNodeMap>>,
    leaves: BlockingMutex<LruCache<Hash, LeafNodeSet>>,
}

impl Cache {
    pub fn new() -> Self {
        const ERROR: &str = "cache capacity must be non-zero";

        Self {
            roots: BlockingMutex::new(HashMap::default()),
            inners: BlockingMutex::new(LruCache::new(
                NonZeroUsize::new(INNERS_CAPACITY).expect(ERROR),
            )),
            leaves: BlockingMutex::new(LruCache::new(
                NonZeroUsize::new(LEAVES_CAPACITY).expect(ERROR),
            )),
        }
    }

    pub fn begin(self: &Arc<Self>) -> CacheTransaction {
        CacheTransaction {
            cache: self.clone(),
            roots_patch: HashMap::default(),
        }
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) struct CacheTransaction {
    cache: Arc<Cache>,
    roots_patch: HashMap<PublicKey, Option<RootNode>>,
}

impl CacheTransaction {
    pub fn put_root(&mut self, node: RootNode) {
        self.roots_patch.insert(node.proof.writer_id, Some(node));
    }

    pub fn remove_root(&mut self, branch_id: &PublicKey) {
        self.roots_patch.insert(*branch_id, None);
    }

    pub fn get_root(&self, branch_id: &PublicKey) -> Option<RootNode> {
        if let Some(node) = self.roots_patch.get(branch_id) {
            node.as_ref().cloned()
        } else {
            self.cache.roots.lock().unwrap().get(branch_id).cloned()
        }
    }

    pub fn get_inners(&self, parent_hash: &Hash) -> Option<InnerNodeMap> {
        self.cache.inners.lock().unwrap().get(parent_hash).cloned()
    }

    pub fn put_inners(&self, parent_hash: Hash, nodes: InnerNodeMap) {
        // NOTE: Writing directly to the cache because this cache entry is immutable
        self.cache.inners.lock().unwrap().put(parent_hash, nodes);
    }

    pub fn get_leaves(&self, parent_hash: &Hash) -> Option<LeafNodeSet> {
        self.cache.leaves.lock().unwrap().get(parent_hash).cloned()
    }

    pub fn put_leaves(&self, parent_hash: Hash, nodes: LeafNodeSet) {
        // NOTE: Writing directly to the cache because this cache entry is immutable
        self.cache.leaves.lock().unwrap().put(parent_hash, nodes);
    }

    pub fn is_dirty(&self) -> bool {
        !self.roots_patch.is_empty()
    }

    pub fn commit(self) {
        let mut roots = self.cache.roots.lock().unwrap();

        for (branch_id, node) in self.roots_patch {
            if let Some(node) = node {
                roots.insert(branch_id, node);
            } else {
                roots.remove(&branch_id);
            }
        }
    }
}

// Max number of leaf node sets in the cache.
const LEAVES_CAPACITY: usize = 1024;

// Max number of inner node maps in the cache. Assuming nodes are uniformly distributed, there
// should be roughly twice as many inner nodes than leaf nodes (for number of leaf nodes < 65536).
const INNERS_CAPACITY: usize = 2 * LEAVES_CAPACITY;

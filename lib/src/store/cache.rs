use crate::{
    collections::HashMap,
    crypto::{sign::PublicKey, Hash},
    deadlock::BlockingMutex,
    protocol::{InnerNodes, LeafNodes, RootNode, Summary},
};
use lru::LruCache;
use std::{num::NonZeroUsize, sync::Arc};

/// Cache for index nodes
pub(super) struct Cache {
    roots: BlockingMutex<HashMap<PublicKey, RootNode>>,
    inners: BlockingMutex<LruCache<Hash, InnerNodes>>,
    leaves: BlockingMutex<LruCache<Hash, LeafNodes>>,
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
            roots: HashMap::default(),
            root_summaries: HashMap::default(),
            inner_summaries: HashMap::default(),
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
    roots: HashMap<PublicKey, Option<RootNode>>,
    root_summaries: HashMap<Hash, Summary>,
    inner_summaries: HashMap<Hash, HashMap<u8, Summary>>,
}

impl CacheTransaction {
    pub fn put_root(&mut self, node: RootNode) {
        self.roots.insert(node.proof.writer_id, Some(node));
    }

    pub fn remove_root(&mut self, branch_id: &PublicKey) {
        self.roots.insert(*branch_id, None);
    }

    pub fn get_root(&self, branch_id: &PublicKey) -> Option<RootNode> {
        let node = if let Some(node) = self.roots.get(branch_id) {
            node.as_ref().cloned()
        } else {
            self.cache.roots.lock().unwrap().get(branch_id).cloned()
        };

        let mut node = node?;

        if let Some(summary) = self.root_summaries.get(&node.proof.hash).copied() {
            node.summary = summary;
        }

        Some(node)
    }

    pub fn update_root_summary(&mut self, hash: Hash, summary: Summary) {
        self.root_summaries.insert(hash, summary);
    }

    pub fn update_inner_summary(&mut self, parent_hash: Hash, bucket: u8, summary: Summary) {
        self.inner_summaries
            .entry(parent_hash)
            .or_default()
            .insert(bucket, summary);
    }

    pub fn get_inners(&self, parent_hash: &Hash) -> Option<InnerNodes> {
        self.cache.inners.lock().unwrap().get(parent_hash).cloned()
    }

    pub fn put_inners(&self, parent_hash: Hash, nodes: InnerNodes) {
        // NOTE: Writing directly to the cache because this cache entry is immutable
        self.cache.inners.lock().unwrap().put(parent_hash, nodes);
    }

    pub fn get_leaves(&self, parent_hash: &Hash) -> Option<LeafNodes> {
        self.cache.leaves.lock().unwrap().get(parent_hash).cloned()
    }

    pub fn put_leaves(&self, parent_hash: Hash, nodes: LeafNodes) {
        // NOTE: Writing directly to the cache because this cache entry is immutable
        self.cache.leaves.lock().unwrap().put(parent_hash, nodes);
    }

    pub fn is_dirty(&self) -> bool {
        !self.roots.is_empty()
            || !self.root_summaries.is_empty()
            || !self.inner_summaries.is_empty()
    }

    pub fn commit(self) {
        {
            let mut roots = self.cache.roots.lock().unwrap();

            for (branch_id, node) in self.roots {
                if let Some(node) = node {
                    roots.insert(branch_id, node);
                } else {
                    roots.remove(&branch_id);
                }
            }

            for node in roots.values_mut() {
                if let Some(summary) = self.root_summaries.get(&node.proof.hash).copied() {
                    node.summary = summary;
                }
            }
        }

        {
            let mut inners = self.cache.inners.lock().unwrap();

            for (parent_hash, summaries) in self.inner_summaries {
                if let Some(nodes) = inners.get_mut(&parent_hash) {
                    for (bucket, summary) in summaries {
                        if let Some(node) = nodes.get_mut(bucket) {
                            node.summary = summary;
                        }
                    }
                }
            }
        }
    }
}

// Max number of leaf node sets in the cache.
const LEAVES_CAPACITY: usize = 1024;

// Max number of inner node maps in the cache. Assuming nodes are uniformly distributed, there
// should be roughly twice as many inner nodes than leaf nodes (for number of leaf nodes < 65536).
const INNERS_CAPACITY: usize = 2 * LEAVES_CAPACITY;

use super::{error::Error, inner_node, leaf_node, root_node, ReadTransaction, WriteTransaction};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hash, Hashable,
    },
    protocol::{
        get_bucket, BlockId, Bump, InnerNode, InnerNodes, LeafNodes, NodeState, Proof,
        RootNodeFilter, RootNodeKind, SingleBlockPresence, Summary, EMPTY_INNER_HASH,
        EMPTY_LEAF_HASH, INNER_LAYER_COUNT,
    },
    version_vector::VersionVector,
};
use std::{
    collections::{btree_map::Entry, BTreeMap},
    fmt,
    ops::Range,
};

/// Helper structure for creating new snapshots in the index. It represents the set of nodes that
/// are different compared to the current snapshot.
pub(super) struct Patch {
    branch_id: PublicKey,
    vv: VersionVector,
    root_hash: Hash,
    root_summary: Summary,
    inners: BTreeMap<Key, InnerNodes>,
    leaves: BTreeMap<Key, LeafNodes>,
}

impl Patch {
    pub async fn new(tx: &mut ReadTransaction, branch_id: PublicKey) -> Result<Self, Error> {
        let (vv, root_hash, root_summary) = match tx
            .load_latest_approved_root_node(&branch_id, RootNodeFilter::Any)
            .await
        {
            Ok(node) => {
                let hash = node.proof.hash;
                (node.proof.into_version_vector(), hash, node.summary)
            }
            Err(Error::BranchNotFound) => {
                (VersionVector::new(), *EMPTY_INNER_HASH, Summary::INCOMPLETE)
            }
            Err(error) => return Err(error),
        };

        Ok(Self {
            branch_id,
            vv,
            root_hash,
            root_summary,
            inners: BTreeMap::new(),
            leaves: BTreeMap::new(),
        })
    }

    pub fn version_vector(&self) -> &VersionVector {
        &self.vv
    }

    pub async fn insert(
        &mut self,
        tx: &mut ReadTransaction,
        encoded_locator: Hash,
        block_id: BlockId,
        block_presence: SingleBlockPresence,
    ) -> Result<bool, Error> {
        let nodes = self.fetch(tx, &encoded_locator).await?;
        let changed = nodes.insert(encoded_locator, block_id, block_presence);

        Ok(changed)
    }

    pub async fn remove(
        &mut self,
        tx: &mut ReadTransaction,
        encoded_locator: &Hash,
        expected_block_id: Option<&BlockId>,
    ) -> Result<bool, Error> {
        let nodes = self.fetch(tx, encoded_locator).await?;

        let old = if let Some(block_id) = expected_block_id {
            nodes.remove_if(encoded_locator, block_id)
        } else {
            nodes.remove(encoded_locator)
        };

        Ok(old.is_some())
    }

    pub async fn save(
        mut self,
        tx: &mut WriteTransaction,
        bump: Bump,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        self.recalculate();
        self.save_children(tx).await?;
        self.save_root(tx, bump, write_keys).await?;

        Ok(())
    }

    async fn fetch<'a>(
        &'a mut self,
        tx: &'_ mut ReadTransaction,
        encoded_locator: &'_ Hash,
    ) -> Result<&'a mut LeafNodes, Error> {
        let mut parent_hash = self.root_hash;
        let mut key = Key::ROOT;

        for layer in 0..INNER_LAYER_COUNT {
            let bucket = get_bucket(encoded_locator, layer);
            let nodes = match self.inners.entry(key) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    entry.insert(tx.load_inner_nodes_with_cache(&parent_hash).await?)
                }
            };

            parent_hash = nodes
                .get(bucket)
                .map(|node| node.hash)
                .unwrap_or_else(|| empty_hash(layer));

            key = key.child(bucket);
        }

        let nodes = match self.leaves.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                entry.insert(tx.load_leaf_nodes_with_cache(&parent_hash).await?)
            }
        };

        Ok(nodes)
    }

    fn recalculate(&mut self) {
        for (key, nodes) in &self.leaves {
            let (parent_key, bucket) = key.parent_and_bucket();
            let parent_nodes = self.inners.entry(parent_key).or_default();

            if !nodes.is_empty() {
                let hash = nodes.hash();
                let summary = Summary::from_leaves(nodes);

                parent_nodes.insert(bucket, InnerNode::new(hash, summary));
            } else {
                parent_nodes.remove(bucket);
            }
        }

        let mut stash = Vec::new();

        for layer in (1..INNER_LAYER_COUNT).rev() {
            for (key, nodes) in self.inners.range(Key::range(layer)) {
                let (parent_key, bucket) = key.parent_and_bucket();

                if !nodes.is_empty() {
                    let hash = nodes.hash();
                    let summary = Summary::from_inners(nodes);

                    stash.push((parent_key, bucket, Some((hash, summary))));
                } else {
                    stash.push((parent_key, bucket, None));
                }
            }

            for (parent_key, bucket, hash_and_summary) in stash.drain(..) {
                let parent_nodes = self.inners.entry(parent_key).or_default();

                if let Some((hash, summary)) = hash_and_summary {
                    parent_nodes.insert(bucket, InnerNode::new(hash, summary));
                } else {
                    parent_nodes.remove(bucket);
                }
            }
        }

        if let Some(nodes) = self.inners.get(&Key::ROOT) {
            self.root_hash = nodes.hash();
            self.root_summary = Summary::from_inners(nodes).with_state(NodeState::Approved);
        }
    }

    async fn save_children(&mut self, tx: &mut WriteTransaction) -> Result<(), Error> {
        let mut stack = vec![(self.root_hash, Key::ROOT)];

        while let Some((parent_hash, key)) = stack.pop() {
            if (key.layer as usize) < INNER_LAYER_COUNT {
                if let Some(nodes) = self.inners.remove(&key) {
                    inner_node::save_all(tx.db(), &nodes, &parent_hash).await?;

                    for (bucket, node) in &nodes {
                        stack.push((node.hash, key.child(bucket)));
                    }

                    tx.inner.inner.cache.put_inners(parent_hash, nodes);
                }
            } else if let Some(nodes) = self.leaves.remove(&key) {
                leaf_node::save_all(tx.db(), &nodes, &parent_hash).await?;
                tx.inner.inner.cache.put_leaves(parent_hash, nodes);
            }
        }

        debug_assert!(self.inners.values().all(|nodes| nodes.is_empty()));
        debug_assert!(self.leaves.values().all(|nodes| nodes.is_empty()));

        Ok(())
    }

    async fn save_root(
        mut self,
        tx: &mut WriteTransaction,
        bump: Bump,
        write_keys: &Keypair,
    ) -> Result<(), Error> {
        let db = tx.db();

        bump.apply(&mut self.vv);

        let new_proof = Proof::new(self.branch_id, self.vv, self.root_hash, write_keys);

        let (root_node, kind) =
            root_node::create(db, new_proof, self.root_summary, RootNodeFilter::Any).await?;

        match kind {
            RootNodeKind::Published => root_node::remove_older(db, &root_node).await?,
            RootNodeKind::Draft => (),
        }

        tracing::trace!(
            vv = ?root_node.proof.version_vector,
            hash = ?root_node.proof.hash,
            branch_id = ?root_node.proof.writer_id,
            block_presence = ?root_node.summary.block_presence,
            "Local snapshot created"
        );

        tx.inner.inner.cache.put_root(root_node);

        Ok(())
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct Key {
    layer: u8,
    path: [u8; INNER_LAYER_COUNT],
}

impl Key {
    const ROOT: Self = Self {
        layer: 0,
        path: [0; INNER_LAYER_COUNT],
    };

    fn child(mut self, bucket: u8) -> Self {
        self.path[self.layer as usize] = bucket;
        self.layer += 1;
        self
    }

    fn parent_and_bucket(mut self) -> (Self, u8) {
        self.layer = self.layer.checked_sub(1).expect("root key has no parent");
        let bucket = self.path[self.layer as usize];
        self.path[self.layer as usize] = 0;

        (self, bucket)
    }

    // Range of Keys that corresponds to all the nodes at the given layer.
    fn range(layer: usize) -> Range<Self> {
        let a = Key {
            layer: layer as u8,
            path: [0; INNER_LAYER_COUNT],
        };
        let b = Key {
            layer: layer as u8 + 1,
            path: [0; INNER_LAYER_COUNT],
        };

        a..b
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", &self.path[..self.layer as usize])
    }
}

fn empty_hash(layer: usize) -> Hash {
    if layer < INNER_LAYER_COUNT - 1 {
        *EMPTY_INNER_HASH
    } else {
        *EMPTY_LEAF_HASH
    }
}

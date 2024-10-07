use futures_util::TryStreamExt as _;

use super::{
    block,
    block_expiration_tracker::BlockExpirationTracker,
    block_id_cache::BlockIdCache,
    block_ids, index, inner_node, leaf_node,
    quota::{self, QuotaError},
    root_node::{self, RootNodeStatus},
    Error,
};
use crate::{
    collections::HashSet,
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    db,
    future::TryStreamExt as _,
    protocol::{
        Block, BlockId, InnerNodes, LeafNodes, MultiBlockPresence, NodeState, Proof,
        RootNodeFilter, SingleBlockPresence, Summary,
    },
    repository, StorageSize,
};
use std::{mem, sync::Arc};

/// Store operations for the client side of the sync protocol.
pub(crate) struct ClientWriter {
    db: db::WriteTransaction,
    block_expiration_tracker: Option<Arc<BlockExpirationTracker>>,
    quota: Option<StorageSize>,
    summary_updates: Vec<Hash>,
    new_blocks: Vec<BlockId>,
    block_id_cache: BlockIdCache,
    block_id_cache_updates: Vec<(Hash, BlockId)>,
}

impl ClientWriter {
    pub(super) async fn begin(
        mut db: db::WriteTransaction,
        block_id_cache: BlockIdCache,
        block_expiration_tracker: Option<Arc<BlockExpirationTracker>>,
    ) -> Result<Self, Error> {
        let quota = repository::quota::get(&mut db).await?;

        Ok(Self {
            db,
            block_expiration_tracker,
            quota,
            summary_updates: Vec::new(),
            new_blocks: Vec::new(),
            block_id_cache,
            block_id_cache_updates: Vec::new(),
        })
    }

    /// Saves received root node into the store.
    pub async fn save_root_node(
        &mut self,
        proof: Proof,
        block_presence: &MultiBlockPresence,
    ) -> Result<RootNodeStatus, Error> {
        let status = root_node::status(&mut self.db, &proof, block_presence).await?;

        if status.write() {
            let (node, _) = root_node::create(
                &mut self.db,
                proof,
                Summary::INCOMPLETE,
                RootNodeFilter::Published,
            )
            .await?;

            self.summary_updates.push(node.proof.hash);
        }

        Ok(status)
    }

    pub async fn save_inner_nodes(
        &mut self,
        nodes: CacheHash<InnerNodes>,
    ) -> Result<Vec<InnerNodeStatus>, Error> {
        let parent_hash = nodes.hash();

        if !index::parent_exists(&mut self.db, &parent_hash).await? {
            return Ok(Vec::new());
        }

        let mut statuses = Vec::with_capacity(nodes.len());
        let nodes = nodes.into_inner();

        for (_, remote_node) in &nodes {
            // Ignore empty nodes (the peer shouldn't have sent us one anyway)
            if remote_node.is_empty() {
                continue;
            }

            let local_node = inner_node::load(&mut self.db, &remote_node.hash).await?;
            let local_node_summary = local_node
                .map(|node| node.summary)
                .unwrap_or(Summary::INCOMPLETE);

            if local_node_summary.is_outdated(&remote_node.summary) {
                statuses.push(InnerNodeStatus {
                    hash: remote_node.hash,
                    local_block_presence: local_node_summary.block_presence,
                    remote_block_presence: remote_node.summary.block_presence,
                });
            }
        }

        let mut nodes = nodes.into_incomplete();
        inner_node::inherit_summaries(&mut self.db, &mut nodes).await?;

        if inner_node::save_all(&mut self.db, &nodes, &parent_hash).await? > 0 {
            self.summary_updates.push(parent_hash);
        }

        Ok(statuses)
    }

    pub async fn save_leaf_nodes(
        &mut self,
        nodes: CacheHash<LeafNodes>,
    ) -> Result<LeafNodesStatus, Error> {
        let parent_hash = nodes.hash();

        if !index::parent_exists(&mut self.db, &parent_hash).await? {
            return Ok(LeafNodesStatus::default());
        }

        let nodes = nodes.into_inner();
        let mut new_block_offers = Vec::new();

        for node in &nodes {
            // Create the block offer only if the block is `Missing` locally and `Present` or
            // `Expired` remotely.
            //
            // If the block is `Expired` locally we *don't* want to register the offer yet. We only
            // register if after the block's been switched to `Missing` (by either requiring it
            // locally or by someone else requesting it from us). On the other hand, if the block
            // is `Expired` remotely we *do* want to create the offer because that causes the
            // remote peer to switch the block to `Missing` and request it from other peers.
            match node.block_presence {
                SingleBlockPresence::Present | SingleBlockPresence::Expired => {
                    match leaf_node::load_block_presence(&mut self.db, &node.block_id).await? {
                        Some(SingleBlockPresence::Missing) | None => {
                            // Missing, expired or not yet stored locally
                            let node_state = if self.quota.is_some() {
                                // OPTIMIZE: the state is the same for all the nodes in `nodes`, so
                                // it only needs to be loaded once.
                                root_node::load_node_state_of_missing(&mut self.db, &node.block_id)
                                    .await?
                            } else {
                                NodeState::Approved
                            };

                            new_block_offers.push((node.block_id, node_state));
                        }
                        Some(SingleBlockPresence::Present | SingleBlockPresence::Expired) => (),
                    }
                }
                SingleBlockPresence::Missing => (),
            };
        }

        if leaf_node::save_all(&mut self.db, &nodes.into_missing(), &parent_hash).await? > 0 {
            self.summary_updates.push(parent_hash);
        }

        Ok(LeafNodesStatus { new_block_offers })
    }

    pub async fn save_block(&mut self, block: &Block) -> Result<(), Error> {
        let updated = {
            let mut updated = false;
            let mut updates = leaf_node::set_present(&mut self.db, &block.id);

            while let Some(update) = updates.try_next().await? {
                self.summary_updates.push(update.parent);
                self.block_id_cache_updates
                    .push((update.encoded_locator, block.id));
                updated = true;
            }

            updated
        };

        if updated {
            block::write(&mut self.db, block).await?;

            if let Some(tracker) = &self.block_expiration_tracker {
                tracker.handle_block_update(&block.id, false);
            }

            self.new_blocks.push(block.id);
        }

        Ok(())
    }

    /// Commit all pending writes and execute the given callback if and only if the commit completes
    /// successfully.
    pub async fn commit_and_then<F, R>(mut self, f: F) -> Result<R, Error>
    where
        F: FnOnce(CommitStatus) -> R + Send + 'static,
        R: Send + 'static,
    {
        let FinalizeStatus {
            approved_branches,
            rejected_branches,
        } = self.finalize_snapshots().await?;

        let approved_missing_blocks = self
            .load_approved_missing_blocks(&approved_branches)
            .await?;

        let Self {
            db,
            new_blocks,
            block_id_cache,
            block_id_cache_updates,
            ..
        } = self;

        let status = CommitStatus {
            approved_branches,
            rejected_branches,
            approved_missing_blocks,
            new_blocks,
        };

        let output = db
            .commit_and_then(move || {
                block_id_cache.set_present(&block_id_cache_updates);
                f(status)
            })
            .await?;

        Ok(output)
    }

    #[cfg(test)]
    pub async fn commit(self) -> Result<CommitStatus, Error> {
        self.commit_and_then(|status| status).await
    }

    async fn finalize_snapshots(&mut self) -> Result<FinalizeStatus, Error> {
        self.summary_updates.sort();
        self.summary_updates.dedup();

        let states =
            index::update_summaries(&mut self.db, mem::take(&mut self.summary_updates)).await?;

        let mut approved_branches = Vec::new();
        let mut rejected_branches = Vec::new();

        for (hash, state) in states {
            match state {
                NodeState::Complete => (),
                NodeState::Approved | NodeState::Incomplete | NodeState::Rejected => continue,
            }

            let approve = if let Some(quota) = self.quota {
                match quota::check(&mut self.db, &hash, quota).await {
                    Ok(()) => true,
                    Err(QuotaError::Exceeded(size)) => {
                        tracing::warn!(?hash, quota = %quota, size = %size, "snapshot rejected - quota exceeded");
                        false
                    }
                    Err(QuotaError::Outdated) => {
                        tracing::debug!(?hash, "snapshot outdated");
                        false
                    }
                    Err(QuotaError::Store(error)) => return Err(error),
                }
            } else {
                true
            };

            if approve {
                // TODO: put node to cache?

                root_node::approve(&mut self.db, &hash)
                    .try_collect_into(&mut approved_branches)
                    .await?;
            } else {
                root_node::reject(&mut self.db, &hash)
                    .try_collect_into(&mut rejected_branches)
                    .await?;
            }
        }

        approved_branches.sort();
        approved_branches.dedup();

        rejected_branches.sort();
        rejected_branches.dedup();

        Ok(FinalizeStatus {
            approved_branches,
            rejected_branches,
        })
    }

    async fn load_approved_missing_blocks(
        &mut self,
        branches: &[PublicKey],
    ) -> Result<HashSet<BlockId>, Error> {
        let mut output = HashSet::default();

        if self.quota.is_some() {
            for branch_id in branches {
                block_ids::missing_block_ids_in_branch(&mut self.db, branch_id)
                    .try_collect_into(&mut output)
                    .await?;
            }
        }

        Ok(output)
    }
}

pub(crate) struct ClientReader {
    db: db::ReadTransaction,
    quota: Option<StorageSize>,
}

impl ClientReader {
    pub(super) async fn begin(mut db: db::ReadTransaction) -> Result<Self, Error> {
        let quota = repository::quota::get(&mut db).await?;

        Ok(Self { db, quota })
    }

    /// Loads the root node state that should be used for offers for the given block.
    ///
    /// NOTE: If quota is enabled, returns the actual root node state. Otherwise returns an assumed
    /// state that depends only on the block presence, for efficiency.
    pub async fn load_effective_root_node_state_for_block(
        &mut self,
        block_id: &BlockId,
    ) -> Result<NodeState, Error> {
        if self.quota.is_some() {
            root_node::load_node_state_of_missing(&mut self.db, block_id).await
        } else {
            match leaf_node::load_block_presence(&mut self.db, block_id).await? {
                Some(SingleBlockPresence::Missing) | Some(SingleBlockPresence::Expired) => {
                    Ok(NodeState::Approved)
                }
                Some(SingleBlockPresence::Present) | None => Ok(NodeState::Rejected),
            }
        }
    }
}

pub(crate) struct InnerNodeStatus {
    pub hash: Hash,
    pub local_block_presence: MultiBlockPresence,
    pub remote_block_presence: MultiBlockPresence,
}

#[derive(Default)]
pub(crate) struct LeafNodesStatus {
    /// New blocks offered by the received nodes together with their root node states.
    pub new_block_offers: Vec<(BlockId, NodeState)>,
}

pub(crate) struct CommitStatus {
    /// Branches that became approved during this commit.
    pub approved_branches: Vec<PublicKey>,
    /// Branches that became rejected due to failed quota check during this commit
    pub rejected_branches: Vec<PublicKey>,
    /// Missing blocks referenced from the newly approved branches.
    pub approved_missing_blocks: HashSet<BlockId>,
    /// Newly written blocks.
    pub new_blocks: Vec<BlockId>,
}

struct FinalizeStatus {
    approved_branches: Vec<PublicKey>,
    rejected_branches: Vec<PublicKey>,
}

#[cfg(test)]
mod tests {
    use super::super::Changeset;
    use super::*;
    use crate::{
        access_control::WriteSecrets,
        protocol::{
            test_utils::Snapshot, BlockContent, MultiBlockPresence, SingleBlockPresence,
            EMPTY_INNER_HASH,
        },
        store::{SnapshotWriter, Store},
        version_vector::VersionVector,
    };
    use futures_util::{StreamExt, TryStreamExt};
    use rand::{rngs::StdRng, Rng, SeedableRng};

    mod future {
        pub use futures_util::future::join;
        pub use std::future::ready;
    }

    #[tokio::test]
    async fn save_valid_empty_root_node() {
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let remote_id = PublicKey::random();
        let secrets = WriteSecrets::random();

        // Initially the remote branch doesn't exist
        assert!(store
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id)
            .try_next()
            .await
            .unwrap()
            .is_none());

        // Save root node received from a remote replica.
        let mut writer = store.begin_client_write().await.unwrap();
        writer
            .save_root_node(
                Proof::new(
                    remote_id,
                    VersionVector::first(remote_id),
                    *EMPTY_INNER_HASH,
                    &secrets.write_keys,
                ),
                &MultiBlockPresence::None,
            )
            .await
            .unwrap();
        let status = writer.commit().await.unwrap();

        assert_eq!(status.approved_branches, [remote_id]);
        assert!(status.approved_missing_blocks.is_empty());
        assert!(status.new_blocks.is_empty());

        // The remote branch now exist.
        assert!(store
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id)
            .try_next()
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn save_duplicate_root_node() {
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let remote_id = PublicKey::random();
        let secrets = WriteSecrets::random();

        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
        let proof = Proof::new(
            remote_id,
            VersionVector::first(remote_id),
            *snapshot.root_hash(),
            &secrets.write_keys,
        );

        // Receive root node for the first time.
        let mut writer = store.begin_client_write().await.unwrap();
        writer
            .save_root_node(proof.clone(), &MultiBlockPresence::None)
            .await
            .unwrap();
        writer.commit().await.unwrap();

        // Receiving it again is a no-op.
        let mut writer = store.begin_client_write().await.unwrap();
        writer
            .save_root_node(proof, &MultiBlockPresence::None)
            .await
            .unwrap();
        let status = writer.commit().await.unwrap();

        assert!(status.approved_branches.is_empty());

        assert_eq!(
            store
                .acquire_read()
                .await
                .unwrap()
                .load_root_nodes_by_writer(&remote_id)
                .filter(|node| future::ready(node.is_ok()))
                .count()
                .await,
            1
        )
    }

    #[tokio::test]
    async fn save_root_node_with_existing_hash() {
        let mut rng = rand::thread_rng();

        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let secrets = WriteSecrets::generate(&mut rng);

        let local_id = PublicKey::generate(&mut rng);
        let remote_id = PublicKey::generate(&mut rng);

        // Create one block locally
        let block: Block = rng.gen();
        let locator = rng.gen();

        let mut tx = store.begin_write().await.unwrap();
        let mut changeset = Changeset::new();

        changeset.link_block(locator, block.id, SingleBlockPresence::Present);
        changeset.write_block(block);
        changeset
            .apply(&mut tx, &local_id, &secrets.write_keys)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // Receive root node with the same hash as the current local one but different writer id.
        let root = store
            .acquire_read()
            .await
            .unwrap()
            .load_latest_approved_root_node(&local_id, RootNodeFilter::Any)
            .await
            .unwrap();

        assert!(root.summary.state.is_approved());
        let root_hash = root.proof.hash;
        let root_vv = root.proof.version_vector.clone();

        let proof = Proof::new(remote_id, root_vv, root_hash, &secrets.write_keys);

        let mut writer = store.begin_client_write().await.unwrap();
        let status = writer
            .save_root_node(proof, &MultiBlockPresence::None)
            .await
            .unwrap();
        assert_eq!(status, RootNodeStatus::Outdated);
        writer.commit().await.unwrap();

        assert!(store
            .acquire_read()
            .await
            .unwrap()
            .load_latest_approved_root_node(&local_id, RootNodeFilter::Any)
            .await
            .unwrap()
            .summary
            .state
            .is_approved());
    }

    #[tokio::test]
    async fn save_bumped_root_node() {
        let mut rng = rand::thread_rng();

        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let secrets = WriteSecrets::generate(&mut rng);

        let branch_id = PublicKey::random();

        let snapshot = Snapshot::generate(&mut rng, 1);
        let vv0 = VersionVector::first(branch_id);

        SnapshotWriter::begin(&store, &snapshot)
            .await
            .save_nodes(&secrets.write_keys, branch_id, vv0.clone())
            .await
            .commit()
            .await;

        let node = store
            .acquire_read()
            .await
            .unwrap()
            .load_latest_approved_root_node(&branch_id, RootNodeFilter::Any)
            .await
            .unwrap();
        assert_eq!(node.proof.version_vector, vv0);
        assert_eq!(node.summary.state, NodeState::Approved);

        // Receive root node with the same hash as before but greater vv.
        let vv1 = vv0.incremented(branch_id);
        let mut writer = store.begin_client_write().await.unwrap();
        writer
            .save_root_node(
                Proof::new(
                    branch_id,
                    vv1.clone(),
                    *snapshot.root_hash(),
                    &secrets.write_keys,
                ),
                &MultiBlockPresence::None,
            )
            .await
            .unwrap();
        writer.commit().await.unwrap();

        let node = store
            .acquire_read()
            .await
            .unwrap()
            .load_latest_approved_root_node(&branch_id, RootNodeFilter::Any)
            .await
            .unwrap();
        assert_eq!(node.proof.version_vector, vv1);
        assert_eq!(node.summary.state, NodeState::Approved);
    }

    mod receive_and_create_root_node {
        use super::*;
        use crate::protocol::Bump;
        use tokio::task;

        #[tokio::test]
        async fn local_then_remove() {
            case(TaskOrder::LocalThenRemote).await
        }

        #[tokio::test]
        async fn remote_then_local() {
            case(TaskOrder::RemoteThenLocal).await
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn concurrent() {
            case(TaskOrder::Concurrent).await
        }

        enum TaskOrder {
            LocalThenRemote,
            RemoteThenLocal,
            Concurrent,
        }

        async fn case(order: TaskOrder) {
            let mut rng = StdRng::seed_from_u64(0);
            let (_base_dir, pool) = db::create_temp().await.unwrap();
            let store = Store::new(pool);
            let secrets = WriteSecrets::random();

            let local_id = PublicKey::generate(&mut rng);

            let locator_0 = rng.gen();
            let block_id_0_0 = rng.gen();
            let block_id_0_1 = rng.gen();

            let locator_1 = rng.gen();
            let block_1: Block = rng.gen();

            let locator_2 = rng.gen();
            let block_id_2 = rng.gen();

            // Insert one present and two missing, so the root block presence is `Some`
            let mut tx = store.begin_write().await.unwrap();
            let mut changeset = Changeset::new();

            for (locator, block_id, presence) in [
                (locator_0, block_id_0_0, SingleBlockPresence::Present),
                (locator_1, block_1.id, SingleBlockPresence::Missing),
                (locator_2, block_id_2, SingleBlockPresence::Missing),
            ] {
                changeset.link_block(locator, block_id, presence);
            }

            changeset
                .apply(&mut tx, &local_id, &secrets.write_keys)
                .await
                .unwrap();
            tx.commit().await.unwrap();

            let root_node_0 = store
                .acquire_read()
                .await
                .unwrap()
                .load_root_nodes_by_writer(&local_id)
                .try_next()
                .await
                .unwrap()
                .unwrap();

            // Mark one of the missing block as present so the block presences are different (but still
            // `Some`).
            let mut writer = store.begin_client_write().await.unwrap();
            writer.save_block(&block_1).await.unwrap();
            writer.commit().await.unwrap();

            // Receive the same node we already have. The hashes and version vectors are equal but the
            // block presences are different (and both are `Some`) so the received node is considered
            // up-to-date.
            let remote_task = async {
                let mut writer = store.begin_client_write().await.unwrap();
                writer
                    .save_root_node(
                        root_node_0.proof.clone(),
                        &root_node_0.summary.block_presence,
                    )
                    .await
                    .unwrap();
                writer.commit().await.unwrap();
            };

            // Create a new snapshot locally
            let local_task = async {
                // This transaction will block `remote_task` until it is committed.
                let mut tx = store.begin_write().await.unwrap();

                // yield a bit to give `remote_task` chance to run until it needs to begin its own
                // transaction.
                for _ in 0..100 {
                    task::yield_now().await;
                }

                let mut changeset = Changeset::new();
                changeset.link_block(locator_0, block_id_0_1, SingleBlockPresence::Present);
                changeset.bump(Bump::increment(local_id));
                changeset
                    .apply(&mut tx, &local_id, &secrets.write_keys)
                    .await
                    .unwrap();

                tx.commit().await.unwrap();
            };

            match order {
                TaskOrder::LocalThenRemote => {
                    local_task.await;
                    remote_task.await;
                }
                TaskOrder::RemoteThenLocal => {
                    remote_task.await;
                    local_task.await;
                }
                TaskOrder::Concurrent => {
                    future::join(remote_task, local_task).await;
                }
            }

            let root_node_1 = store
                .acquire_read()
                .await
                .unwrap()
                .load_root_nodes_by_writer(&local_id)
                .try_next()
                .await
                .unwrap()
                .unwrap();

            // In all three cases the locally created snapshot must be newer than the received one:
            // - In the local-then-remote case, the remote is outdated by the time it's received and so
            //   it's not even inserted.
            // - In the remote-then-local case, the remote one is inserted first but then the local one
            //   overwrites it
            // - In the concurrent case, the remote is still up-to-date when its started to get received
            //   but the local is holding a db transaction so the remote can't proceed until the local one
            //   commits it, and so by the time the transaction is committed, the remote is no longer
            //   up-to-date.
            assert!(root_node_1.proof.version_vector > root_node_0.proof.version_vector);
        }
    }

    #[tokio::test]
    async fn save_valid_child_nodes() {
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let secrets = WriteSecrets::random();
        let remote_id = PublicKey::random();

        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

        let mut writer = store.begin_client_write().await.unwrap();
        writer
            .save_root_node(
                Proof::new(
                    remote_id,
                    VersionVector::first(remote_id),
                    *snapshot.root_hash(),
                    &secrets.write_keys,
                ),
                &MultiBlockPresence::None,
            )
            .await
            .unwrap();
        writer.commit().await.unwrap();

        for (hash, inner_nodes) in snapshot.inner_sets() {
            let mut writer = store.begin_client_write().await.unwrap();
            writer
                .save_inner_nodes(inner_nodes.clone().into())
                .await
                .unwrap();
            writer.commit().await.unwrap();

            assert!(!store
                .acquire_read()
                .await
                .unwrap()
                .load_inner_nodes(hash)
                .await
                .unwrap()
                .is_empty());
        }

        for (hash, leaf_nodes) in snapshot.leaf_sets() {
            let mut writer = store.begin_client_write().await.unwrap();
            writer
                .save_leaf_nodes(leaf_nodes.clone().into())
                .await
                .unwrap();
            writer.commit().await.unwrap();

            assert!(!store
                .acquire_read()
                .await
                .unwrap()
                .load_leaf_nodes(hash)
                .await
                .unwrap()
                .is_empty());
        }
    }

    #[tokio::test]
    async fn save_child_nodes_with_missing_root_parent() {
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

        // Try to save the inner nodes
        let (hash, inner_nodes) = snapshot.inner_sets().next().unwrap();
        let mut writer = store.begin_client_write().await.unwrap();
        let statuses = writer
            .save_inner_nodes(inner_nodes.clone().into())
            .await
            .unwrap();
        assert!(statuses.is_empty());
        writer.commit().await.unwrap();

        // The orphaned inner nodes were not written to the db.
        let inner_nodes = store
            .acquire_read()
            .await
            .unwrap()
            .load_inner_nodes(hash)
            .await
            .unwrap();
        assert!(inner_nodes.is_empty());

        // Try to save the leaf nodes
        let (hash, leaf_nodes) = snapshot.leaf_sets().next().unwrap();
        let mut writer = store.begin_client_write().await.unwrap();

        let status = writer
            .save_leaf_nodes(leaf_nodes.clone().into())
            .await
            .unwrap();
        assert!(status.new_block_offers.is_empty());

        let status = writer.commit().await.unwrap();
        assert!(status.approved_branches.is_empty());

        // The orphaned leaf nodes were not written to the db.
        let leaf_nodes = store
            .acquire_read()
            .await
            .unwrap()
            .load_leaf_nodes(hash)
            .await
            .unwrap();
        assert!(leaf_nodes.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn save_valid_blocks() {
        crate::test_utils::init_log();

        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let secrets = WriteSecrets::random();
        let branch_id = PublicKey::random();
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 5);

        SnapshotWriter::begin(&store, &snapshot)
            .await
            .save_nodes(
                &secrets.write_keys,
                branch_id,
                VersionVector::first(branch_id),
            )
            .await
            .save_blocks()
            .await
            .commit()
            .await;

        let mut reader = store.acquire_read().await.unwrap();

        for (id, block) in snapshot.blocks() {
            let mut content = BlockContent::new();
            let nonce = reader.read_block(id, &mut content).await.unwrap();

            assert_eq!(&content[..], &block.content[..]);
            assert_eq!(nonce, block.nonce);
            assert_eq!(BlockId::new(&content, &nonce), *id);
        }
    }

    #[tokio::test]
    async fn save_orphaned_block() {
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

        let mut writer = store.begin_client_write().await.unwrap();
        for block in snapshot.blocks().values() {
            writer.save_block(block).await.unwrap();
        }
        writer.commit().await.unwrap();

        let mut reader = store.acquire_read().await.unwrap();
        for id in snapshot.blocks().keys() {
            assert!(!reader.block_exists(id).await.unwrap());
        }
    }

    #[tokio::test]
    async fn update_incomplete_snapshot_by_removing_blocks() {
        let mut rng = rand::thread_rng();
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let store = Store::new(pool);
        let secrets = WriteSecrets::generate(&mut rng);
        let branch_id = PublicKey::generate(&mut rng);
        let snapshot = Snapshot::generate(&mut rng, 2);

        let block_to_remove = snapshot.blocks().keys().copied().next().unwrap();

        let vv1 = VersionVector::first(branch_id);
        let mut writer = SnapshotWriter::begin(&store, &snapshot)
            .await
            .save_root_nodes(&secrets.write_keys, branch_id, vv1.clone())
            .await
            .save_inner_nodes()
            .await;

        // Skip the leaf nodes that point to the block to be removed.
        for (_, nodes) in snapshot.leaf_sets() {
            if nodes.iter().any(|node| node.block_id == block_to_remove) {
                continue;
            }

            writer
                .client_writer()
                .save_leaf_nodes(nodes.clone().into())
                .await
                .unwrap();
        }

        writer.commit().await;

        // Verify the latest snapshot is incomplete
        let node = store
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&branch_id)
            .try_next()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(node.summary.state, NodeState::Incomplete);

        // Remove the block
        let snapshot = Snapshot::from_present_blocks(
            snapshot
                .locators_and_blocks()
                .filter(|(_, block)| block.id != block_to_remove)
                .map(|(locator, block)| (*locator, block.clone())),
        );

        let vv2 = vv1.incremented(branch_id);
        SnapshotWriter::begin(&store, &snapshot)
            .await
            .save_nodes(&secrets.write_keys, branch_id, vv2)
            .await
            .commit()
            .await;

        // Verify the latest snapshot is now complete
        let node = store
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&branch_id)
            .try_next()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(node.summary.state, NodeState::Approved);
    }
}

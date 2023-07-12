//! Repository state and operations that don't require read or write access.

use super::{quota, LocalId, Metadata, RepositoryId, RepositoryMonitor};
use crate::{
    block::{tracker::BlockPromise, BlockData, BlockNonce, BlockTracker},
    crypto::{sign::PublicKey, CacheHash},
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    event::{EventSender, Payload},
    index::{MultiBlockPresence, ProofError, UntrustedProof},
    storage_size::StorageSize,
    store::{
        self, InnerNodeMap, InnerNodeReceiveStatus, LeafNodeReceiveStatus, LeafNodeSet,
        ReceiveFilter, RootNodeReceiveStatus, Store, WriteTransaction,
    },
};
use futures_util::TryStreamExt;
use sqlx::Row;
use std::sync::Arc;
use tracing::Level;

#[derive(Clone)]
pub(crate) struct Vault {
    pub repository_id: RepositoryId,
    pub store: Store,
    pub event_tx: EventSender,
    pub block_tracker: BlockTracker,
    pub block_request_mode: BlockRequestMode,
    pub local_id: LocalId,
    pub monitor: Arc<RepositoryMonitor>,
}

impl Vault {
    pub fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    /// Receive `RootNode` from other replica and store it into the db. Returns whether the
    /// received node has any new information compared to all the nodes already stored locally.
    pub async fn receive_root_node(
        &self,
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
    ) -> Result<RootNodeReceiveStatus> {
        let proof = match proof.verify(self.repository_id()) {
            Ok(proof) => proof,
            Err(ProofError(proof)) => {
                tracing::trace!(branch_id = ?proof.writer_id, hash = ?proof.hash, "invalid proof");
                return Ok(RootNodeReceiveStatus::default());
            }
        };

        // Ignore branches with empty version vectors because they have no content yet.
        if proof.version_vector.is_empty() {
            return Ok(RootNodeReceiveStatus::default());
        }

        let mut tx = self.store().begin_write().await?;
        let status = tx.receive_root_node(proof, block_presence).await?;
        self.finalize_receive(tx, &status.new_approved).await?;

        Ok(status)
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    /// Also returns the receive status.
    pub async fn receive_inner_nodes(
        &self,
        nodes: CacheHash<InnerNodeMap>,
        receive_filter: &ReceiveFilter,
        quota: Option<StorageSize>,
    ) -> Result<InnerNodeReceiveStatus> {
        let mut tx = self.store().begin_write().await?;
        let status = tx.receive_inner_nodes(nodes, receive_filter, quota).await?;
        self.finalize_receive(tx, &status.new_approved).await?;

        Ok(status)
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    /// Also returns the receive status.
    pub async fn receive_leaf_nodes(
        &self,
        nodes: CacheHash<LeafNodeSet>,
        quota: Option<StorageSize>,
    ) -> Result<LeafNodeReceiveStatus> {
        let mut tx = self.store().begin_write().await?;
        let status = tx.receive_leaf_nodes(nodes, quota).await?;
        self.finalize_receive(tx, &status.new_approved).await?;

        Ok(status)
    }

    /// Receive a block from other replica.
    pub async fn receive_block(
        &self,
        data: &BlockData,
        nonce: &BlockNonce,
        promise: Option<BlockPromise>,
    ) -> Result<()> {
        let block_id = data.id;
        let event_tx = self.event_tx.clone();

        let mut tx = self.store().begin_write().await?;
        let status = match tx.receive_block(data, nonce).await {
            Ok(status) => status,
            Err(error) => {
                if matches!(error, store::Error::BlockNotReferenced) {
                    // We no longer need this block but we still need to un-track it.
                    if let Some(promise) = promise {
                        promise.complete();
                    }
                }

                return Err(error.into());
            }
        };

        tx.commit_and_then(move || {
            // Notify affected branches.
            for branch_id in status.branches {
                event_tx.send(Payload::BlockReceived {
                    block_id,
                    branch_id,
                });
            }

            if let Some(promise) = promise {
                promise.complete();
            }
        })
        .await?;

        Ok(())
    }

    pub fn metadata(&self) -> Metadata {
        Metadata::new(self.store().raw().clone())
    }

    /// Total size of the stored data
    pub async fn size(&self) -> Result<StorageSize> {
        let mut conn = self.store().raw().acquire().await?;

        // Note: for simplicity, we are currently counting only blocks (content + id + nonce)
        let count = db::decode_u64(
            sqlx::query("SELECT COUNT(*) FROM blocks")
                .fetch_one(&mut *conn)
                .await?
                .get(0),
        );

        Ok(StorageSize::from_blocks(count))
    }

    pub async fn set_quota(&self, quota: Option<StorageSize>) -> Result<()> {
        let mut tx = self.store().raw().begin_write().await?;

        if let Some(quota) = quota {
            quota::set(&mut tx, quota.to_bytes()).await?
        } else {
            quota::remove(&mut tx).await?
        }

        tx.commit().await?;

        Ok(())
    }

    pub async fn quota(&self) -> Result<Option<StorageSize>> {
        let mut conn = self.store().raw().acquire().await?;
        match quota::get(&mut conn).await {
            Ok(quota) => Ok(Some(StorageSize::from_bytes(quota))),
            Err(Error::EntryNotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn approve_offers(&self, branch_id: &PublicKey) -> Result<()> {
        let mut tx = self.store().begin_read().await?;
        let mut block_ids = tx.missing_block_ids_in_branch(branch_id);

        while let Some(block_id) = block_ids.try_next().await? {
            self.block_tracker.approve(&block_id);
        }

        Ok(())
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        self.store().debug_print_root_node(print).await
    }

    // Finalizes receiving nodes from a remote replica, commits the transaction and notifies the
    // affected branches.
    async fn finalize_receive(
        &self,
        mut tx: WriteTransaction,
        new_approved: &[PublicKey],
    ) -> Result<()> {
        // For logging completed snapshots
        let root_nodes = if tracing::enabled!(Level::DEBUG) {
            let mut root_nodes = Vec::with_capacity(new_approved.len());

            for branch_id in new_approved {
                root_nodes.push(tx.load_root_node(branch_id).await?);
            }

            root_nodes
        } else {
            Vec::new()
        };

        tx.commit_and_then({
            let new_approved = new_approved.to_vec();
            let event_tx = self.event_tx.clone();

            move || {
                for root_node in root_nodes {
                    tracing::debug!(
                        branch_id = ?root_node.proof.writer_id,
                        hash = ?root_node.proof.hash,
                        vv = ?root_node.proof.version_vector,
                        "snapshot complete"
                    );
                }

                for branch_id in new_approved {
                    event_tx.send(Payload::BranchChanged(branch_id));
                }
            }
        })
        .await?;

        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(crate) enum BlockRequestMode {
    // Request only required blocks
    Lazy,
    // Request all blocks
    Greedy,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crypto::{
            cipher::SecretKey,
            sign::{Keypair, PublicKey},
            Hash,
        },
        db,
        event::EventSender,
        index::{
            node_test_utils::{receive_nodes, Block, Snapshot},
            MultiBlockPresence, Proof, SingleBlockPresence,
        },
        locator::Locator,
        metrics::Metrics,
        repository::RepositoryId,
        state_monitor::StateMonitor,
        store::Store,
        version_vector::VersionVector,
    };
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_local() {
        let (_base_dir, pool) = setup().await;
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();
        let vault = create_vault(pool, RepositoryId::from(write_keys.public));

        let branch_id = PublicKey::random();

        let locator = Locator::head(rand::random());
        let locator = locator.encode(&read_key);
        let block_id = rand::random();

        let mut tx = vault.store().begin_write().await.unwrap();
        tx.link_block(
            &branch_id,
            &locator,
            &block_id,
            SingleBlockPresence::Present,
            &write_keys,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
        let expected = [block_id].into_iter().collect();

        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_remote() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let vault = create_vault(pool, RepositoryId::from(write_keys.public));
        let receive_filter = vault.store().receive_filter();

        let branch_id = PublicKey::random();
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

        receive_nodes(
            &vault,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
            &receive_filter,
            &snapshot,
        )
        .await;

        let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
        let expected = snapshot.blocks().keys().copied().collect();

        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_excludes_blocks_from_incomplete_snapshots() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let vault = create_vault(pool, RepositoryId::from(write_keys.public));

        let branch_id = PublicKey::random();

        // Create snapshot with two leaf nodes but receive only one of them.
        let snapshot = loop {
            let snapshot = Snapshot::generate(&mut rand::thread_rng(), 2);
            if snapshot.leaf_sets().count() > 1 {
                break snapshot;
            }
        };

        let receive_filter = vault.store().receive_filter();
        let version_vector = VersionVector::first(branch_id);
        let proof = Proof::new(
            branch_id,
            version_vector,
            *snapshot.root_hash(),
            &write_keys,
        );

        vault
            .receive_root_node(proof.into(), MultiBlockPresence::None)
            .await
            .unwrap();

        for layer in snapshot.inner_layers() {
            for (_, nodes) in layer.inner_maps() {
                vault
                    .receive_inner_nodes(nodes.clone().into(), &receive_filter, None)
                    .await
                    .unwrap();
            }
        }

        for (_, nodes) in snapshot.leaf_sets().take(1) {
            vault
                .receive_leaf_nodes(nodes.clone().into(), None)
                .await
                .unwrap();
        }

        let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
        assert!(actual.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_multiple_branches() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let vault = create_vault(pool, RepositoryId::from(write_keys.public));
        let receive_filter = vault.store().receive_filter();

        let branch_id_0 = PublicKey::random();
        let branch_id_1 = PublicKey::random();

        // One block is common between both branches and one for each branch is unique.
        let all_blocks: [(Hash, Block); 3] = rand::random();
        let blocks_0 = &all_blocks[..2];
        let blocks_1 = &all_blocks[1..];

        let snapshot_0 = Snapshot::new(blocks_0.iter().cloned());
        let snapshot_1 = Snapshot::new(blocks_1.iter().cloned());

        receive_nodes(
            &vault,
            &write_keys,
            branch_id_0,
            VersionVector::first(branch_id_0),
            &receive_filter,
            &snapshot_0,
        )
        .await;

        receive_nodes(
            &vault,
            &write_keys,
            branch_id_1,
            VersionVector::first(branch_id_1),
            &receive_filter,
            &snapshot_1,
        )
        .await;

        let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
        let expected = all_blocks
            .iter()
            .map(|(_, block)| block.id())
            .copied()
            .collect();

        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn block_ids_pagination() {
        let (_base_dir, pool) = setup().await;
        let write_keys = Keypair::random();
        let vault = create_vault(pool, RepositoryId::from(write_keys.public));
        let receive_filter = vault.store().receive_filter();

        let branch_id = PublicKey::random();
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 3);

        receive_nodes(
            &vault,
            &write_keys,
            branch_id,
            VersionVector::first(branch_id),
            &receive_filter,
            &snapshot,
        )
        .await;

        let mut sorted_blocks: Vec<_> = snapshot.blocks().keys().copied().collect();
        sorted_blocks.sort();

        let mut page = vault.store.block_ids(2);

        let actual = page.next().await.unwrap();
        let expected = sorted_blocks[..2].iter().copied().collect();
        assert_eq!(actual, expected);

        let actual = page.next().await.unwrap();
        let expected = sorted_blocks[2..].iter().copied().collect();
        assert_eq!(actual, expected);

        let actual = page.next().await.unwrap();
        assert!(actual.is_empty());
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }

    fn create_vault(pool: db::Pool, repository_id: RepositoryId) -> Vault {
        let event_tx = EventSender::new(1);
        let store = Store::new(pool);
        Vault {
            repository_id,
            store,
            event_tx,
            block_tracker: BlockTracker::new(),
            block_request_mode: BlockRequestMode::Lazy,
            local_id: LocalId::new(),
            monitor: Arc::new(RepositoryMonitor::new(
                StateMonitor::make_root(),
                Metrics::new(),
                "test",
            )),
        }
    }
}

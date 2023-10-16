//! Repository state and operations that don't require read or write access.

use super::{quota, LocalId, Metadata, RepositoryId, RepositoryMonitor};
use crate::{
    crypto::{sign::PublicKey, CacheHash},
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    event::{EventSender, Payload},
    missing_parts::{PartPromise, Tracker},
    protocol::{
        Block, BlockId, InnerNodeMap, LeafNodeSet, MultiBlockPresence, ProofError, UntrustedProof,
    },
    storage_size::StorageSize,
    store::{
        self, InnerNodeReceiveStatus, LeafNodeReceiveStatus, ReceiveFilter, RootNodeReceiveStatus,
        Store, WriteTransaction,
    },
};
use futures_util::TryStreamExt;
use sqlx::Row;
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct Vault {
    repository_id: RepositoryId,
    store: Store,
    pub event_tx: EventSender,
    pub parts_tracker: Tracker,
    pub block_request_mode: BlockRequestMode,
    pub local_id: LocalId,
    pub monitor: Arc<RepositoryMonitor>,
}

impl Vault {
    pub fn new(
        repository_id: RepositoryId,
        event_tx: EventSender,
        pool: db::Pool,
        block_request_mode: BlockRequestMode,
        monitor: RepositoryMonitor,
    ) -> Self {
        let store = Store::new(pool);

        Self {
            repository_id,
            store,
            event_tx,
            parts_tracker: Tracker::new(),
            block_request_mode,
            local_id: LocalId::new(),
            monitor: Arc::new(monitor),
        }
    }

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
                tracing::trace!(branch_id = ?proof.writer_id, hash = ?proof.hash, "Invalid proof");
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
    pub async fn receive_block(&self, block: &Block, promise: Option<PartPromise>) -> Result<()> {
        let block_id = block.id;
        let event_tx = self.event_tx.clone();

        let mut tx = self.store().begin_write().await?;
        let status = match tx.receive_block(block).await {
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

    /// Receive a message that the block has been found on the peer.
    pub async fn receive_block_not_found(
        &self,
        block_id: BlockId,
        receive_filter: &ReceiveFilter,
    ) -> Result<()> {
        // We received a 'block not found' because we sent a request for the block, and we sent
        // that request because the index that we downloaded from the peer indicated that the peer
        // had the block. But it could have been lying and the block at the peer could have
        // expired. If that's the case, then the peer should have updated their index and we'll
        // need to re-download the part referring to the `block_id`. Thus we need to remove that
        // part from the `receive_filter`.

        self.store()
            .begin_write()
            .await?
            .remove_from_receive_filter_index_nodes_for(block_id, receive_filter)
            .await?;

        Ok(())
    }

    pub fn metadata(&self) -> Metadata {
        Metadata::new(self.store().db().clone())
    }

    /// Total size of the stored data
    pub async fn size(&self) -> Result<StorageSize> {
        let mut conn = self.store().db().acquire().await?;

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
        let mut tx = self.store().db().begin_write().await?;

        if let Some(quota) = quota {
            quota::set(&mut tx, quota.to_bytes()).await?
        } else {
            quota::remove(&mut tx).await?
        }

        tx.commit().await?;

        Ok(())
    }

    pub async fn quota(&self) -> Result<Option<StorageSize>> {
        let mut conn = self.store().db().acquire().await?;
        match quota::get(&mut conn).await {
            Ok(quota) => Ok(Some(StorageSize::from_bytes(quota))),
            Err(Error::EntryNotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn set_block_expiration(&self, duration: Option<Duration>) -> Result<()> {
        Ok(self
            .store
            .set_block_expiration(duration, self.parts_tracker.clone())
            .await?)
    }

    pub async fn block_expiration(&self) -> Option<Duration> {
        self.store.block_expiration().await
    }

    pub async fn approve_offers(&self, branch_id: &PublicKey) -> Result<()> {
        let mut tx = self.store().begin_read().await?;
        let mut block_ids = tx.missing_block_ids_in_branch(branch_id);

        while let Some(block_id) = block_ids.try_next().await? {
            self.parts_tracker.approve_block(block_id);
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
        tx: WriteTransaction,
        new_approved: &[PublicKey],
    ) -> Result<()> {
        tx.commit_and_then({
            let new_approved = new_approved.to_vec();
            let event_tx = self.event_tx.clone();

            move || {
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

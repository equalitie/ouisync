mod node;
mod proof;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use self::node::test_utils as node_test_utils;
pub(crate) use self::{
    node::{
        receive_block, update_summaries, MultiBlockPresence, NodeState, SingleBlockPresence,
        Summary, UpdateSummaryReason,
    },
    proof::{Proof, UntrustedProof},
};

use self::proof::ProofError;
use crate::{
    crypto::{sign::PublicKey, CacheHash},
    debug::DebugPrinter,
    error::Result,
    event::{Event, EventSender, Payload},
    future::try_collect_into,
    repository::RepositoryId,
    storage_size::StorageSize,
    store::{
        InnerNodeMap, InnerNodeReceiveStatus, LeafNodeReceiveStatus, LeafNodeSet, ReceiveFilter,
        RootNode, RootNodeReceiveStatus, Store, WriteTransaction,
    },
    version_vector::VersionVector,
};
use tokio::sync::broadcast;
use tracing::Level;

pub(crate) type SnapshotId = u32;

#[derive(Clone)]
pub(crate) struct Index {
    store: Store,
    repository_id: RepositoryId,
    event_tx: EventSender,
}

impl Index {
    pub fn new(store: Store, repository_id: RepositoryId, event_tx: EventSender) -> Self {
        Self {
            store,
            repository_id,
            event_tx,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }

    /// Subscribe to change notification from all current and future branches.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    pub(crate) fn notify(&self) -> &EventSender {
        &self.event_tx
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        let mut reader = self.store().acquire_read().await.unwrap();
        RootNode::debug_print(reader.raw_mut(), print).await;
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
            let event_tx = self.notify().clone();

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

/// Operation on version vector
#[derive(Clone, Copy, Debug)]
pub(crate) enum VersionVectorOp<'a> {
    IncrementLocal,
    Merge(&'a VersionVector),
}

impl VersionVectorOp<'_> {
    pub fn apply(self, local_id: &PublicKey, target: &mut VersionVector) {
        match self {
            Self::IncrementLocal => {
                target.increment(*local_id);
            }
            Self::Merge(other) => {
                target.merge(other);
            }
        }
    }
}

mod branch_data;
mod node;
mod path;
mod proof;
mod quota;
mod receive_filter;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use self::node::{test_utils as node_test_utils, EMPTY_INNER_HASH};
pub(crate) use self::{
    branch_data::{BranchData, SnapshotData},
    node::{
        receive_block, update_summaries, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet,
        MultiBlockPresence, NodeState, RootNode, SingleBlockPresence, Summary, UpdateSummaryReason,
    },
    proof::{Proof, UntrustedProof},
    receive_filter::ReceiveFilter,
};

use self::{proof::ProofError, quota::QuotaError};
use crate::{
    block::BlockId,
    crypto::{sign::PublicKey, CacheHash, Hash, Hashable},
    db,
    debug::DebugPrinter,
    error::{Error, Result},
    event::{Event, EventSender, Payload},
    repository::RepositoryId,
    storage_size::StorageSize,
    version_vector::VersionVector,
};
use futures_util::{Stream, TryStreamExt};
use std::{cmp::Ordering, future, iter};
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::Level;

pub(crate) type SnapshotId = u32;

#[derive(Clone)]
pub(crate) struct Index {
    pub pool: db::Pool,
    repository_id: RepositoryId,
    event_tx: EventSender,
}

impl Index {
    pub fn new(pool: db::Pool, repository_id: RepositoryId, event_tx: EventSender) -> Self {
        Self {
            pool,
            repository_id,
            event_tx,
        }
    }

    pub fn repository_id(&self) -> &RepositoryId {
        &self.repository_id
    }

    pub async fn load_branches(&self) -> Result<Vec<BranchData>> {
        let mut conn = self.pool.acquire().await?;
        BranchData::load_all(&mut conn).try_collect().await
    }

    /// Load latest snapshots of all branches.
    pub async fn load_snapshots(&self) -> Result<Vec<SnapshotData>> {
        let mut conn = self.pool.acquire().await?;
        SnapshotData::load_all(&mut conn).try_collect().await
    }

    /// Subscribe to change notification from all current and future branches.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    pub(crate) fn notify(&self) -> &EventSender {
        &self.event_tx
    }

    pub async fn debug_print(&self, print: DebugPrinter) {
        let mut conn = self.pool.acquire().await.unwrap();
        RootNode::debug_print(&mut conn, print).await;
    }

    /// Receive `RootNode` from other replica and store it into the db. Returns whether the
    /// received node has any new information compared to all the nodes already stored locally.
    pub async fn receive_root_node(
        &self,
        proof: UntrustedProof,
        block_presence: MultiBlockPresence,
    ) -> Result<bool, ReceiveError> {
        let proof = proof.verify(self.repository_id())?;

        // Ignore branches with empty version vectors because they have no content yet.
        if proof.version_vector.is_empty() {
            return Ok(false);
        }

        // Make sure the loading of the existing nodes and the potential creation of the new node
        // happens atomically. Otherwise we could conclude the incoming node is up-to-date but
        // just before we start inserting it another snapshot might get created locally and the
        // incoming node might become outdated. But because we already concluded it's up-to-date,
        // we end up inserting it anyway which breaks the invariant that a node inserted later must
        // be happens-after any node inserted earlier in the same branch.
        let mut tx = self.pool.begin_write().await?;

        // Determine further actions by comparing the incoming node against the existing nodes:
        let action = decide_root_node_action(&mut tx, &proof, &block_presence).await?;

        if action.insert {
            let node = RootNode::create(&mut tx, proof, Summary::INCOMPLETE).await?;

            tracing::debug!(
                branch_id = ?node.proof.writer_id,
                hash = ?node.proof.hash,
                vv = ?node.proof.version_vector,
                "snapshot started"
            );

            // Ignoring quota here because if the snapshot became complete by receiving this root
            // node it means that we already have all the other nodes and so the quota validation
            // already took place.
            self.finalize_receive(tx, node.proof.hash, None).await?;
        }

        Ok(action.request_children)
    }

    /// Receive inner nodes from other replica and store them into the db.
    /// Returns hashes of those nodes that were more up to date than the locally stored ones.
    /// Also returns the receive status.
    pub async fn receive_inner_nodes(
        &self,
        nodes: CacheHash<InnerNodeMap>,
        receive_filter: &ReceiveFilter,
        quota: Option<StorageSize>,
    ) -> Result<(Vec<InnerNode>, ReceiveStatus), ReceiveError> {
        let mut tx = self.pool.begin_write().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut tx, &parent_hash).await?;

        let updated_nodes = self
            .find_inner_nodes_with_new_blocks(&mut tx, &nodes, receive_filter)
            .await?;

        let mut nodes = nodes.into_inner().into_incomplete();
        nodes.inherit_summaries(&mut tx).await?;
        nodes.save(&mut tx, &parent_hash).await?;

        let status = self.finalize_receive(tx, parent_hash, quota).await?;

        Ok((updated_nodes, status))
    }

    /// Receive leaf nodes from other replica and store them into the db.
    /// Returns the ids of the blocks that the remote replica has but the local one has not.
    /// Also returns the receive status.
    pub async fn receive_leaf_nodes(
        &self,
        nodes: CacheHash<LeafNodeSet>,
        quota: Option<StorageSize>,
    ) -> Result<(Vec<BlockId>, ReceiveStatus), ReceiveError> {
        let mut tx = self.pool.begin_write().await?;
        let parent_hash = nodes.hash();

        self.check_parent_node_exists(&mut tx, &parent_hash).await?;

        let updated_blocks = self
            .find_leaf_nodes_with_new_blocks(&mut tx, &nodes)
            .await?;

        nodes
            .into_inner()
            .into_missing()
            .save(&mut tx, &parent_hash)
            .await?;

        let status = self.finalize_receive(tx, parent_hash, quota).await?;

        Ok((updated_blocks, status))
    }

    // Filter inner nodes that the remote replica has some blocks in that the local one is missing.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_inner_nodes_with_new_blocks(
        &self,
        tx: &mut db::WriteTransaction,
        remote_nodes: &InnerNodeMap,
        receive_filter: &ReceiveFilter,
    ) -> Result<Vec<InnerNode>> {
        let mut output = Vec::with_capacity(remote_nodes.len());

        for (_, remote_node) in remote_nodes {
            if !receive_filter
                .check(tx, &remote_node.hash, &remote_node.summary.block_presence)
                .await?
            {
                continue;
            }

            let local_node = InnerNode::load(tx, &remote_node.hash).await?;
            let insert = if let Some(local_node) = local_node {
                local_node.summary.is_outdated(&remote_node.summary)
            } else {
                // node not present locally - we implicitly treat this as if the local replica
                // had zero blocks under this node unless the remote node is empty, in that
                // case we ignore it.
                !remote_node.is_empty()
            };

            if insert {
                output.push(*remote_node);
            }
        }

        Ok(output)
    }

    // Filter leaf nodes that the remote replica has a block for but the local one is missing it.
    //
    // Assumes (but does not enforce) that `parent_hash` is the parent hash of all nodes in
    // `remote_nodes`.
    async fn find_leaf_nodes_with_new_blocks(
        &self,
        conn: &mut db::Connection,
        remote_nodes: &LeafNodeSet,
    ) -> Result<Vec<BlockId>> {
        let mut output = Vec::new();

        for remote_node in remote_nodes.present() {
            if !LeafNode::is_present(conn, &remote_node.block_id).await? {
                output.push(remote_node.block_id);
            }
        }

        Ok(output)
    }

    // Finalizes receiving nodes from a remote replica, commits the transaction and notifies the
    // affected branches.
    async fn finalize_receive(
        &self,
        mut tx: db::WriteTransaction,
        hash: Hash,
        quota: Option<StorageSize>,
    ) -> Result<ReceiveStatus> {
        // TODO: Don't hold write transaction through this whole function. Use it only for
        // `update_summaries` then commit it, then do the quota check with a read-only transaction
        // and then grab another write transaction to do the `approve` / `reject`.
        // CAVEAT: the quota check would need some kind of unique lock to prevent multiple
        // concurrent checks to succeed where they would otherwise fail if ran sequentially.

        let states =
            node::update_summaries(&mut tx, vec![hash], UpdateSummaryReason::Other).await?;

        let mut old_approved = false;
        let mut new_approved = Vec::new();

        for (hash, state) in states {
            match state {
                NodeState::Complete => (),
                NodeState::Approved => {
                    old_approved = true;
                    continue;
                }
                NodeState::Incomplete | NodeState::Rejected => continue,
            }

            let approve = if let Some(quota) = quota {
                match quota::check(&mut tx, &hash, quota).await {
                    Ok(()) => true,
                    Err(QuotaError::Exceeded(size)) => {
                        tracing::warn!(?hash, quota = %quota, size = %size, "snapshot rejected - quota exceeded");
                        false
                    }
                    Err(QuotaError::Outdated) => {
                        tracing::debug!(?hash, "snapshot outdated");
                        false
                    }
                    Err(QuotaError::Fatal(error)) => return Err(error),
                }
            } else {
                true
            };

            if approve {
                RootNode::approve(&mut tx, &hash).await?;
                try_collect_into(RootNode::load_writer_ids(&mut tx, &hash), &mut new_approved)
                    .await?;
            } else {
                RootNode::reject(&mut tx, &hash).await?;
            }
        }

        // For logging completed snapshots
        let snapshots = if tracing::enabled!(Level::DEBUG) {
            let mut snapshots = Vec::with_capacity(new_approved.len());

            for branch_id in &new_approved {
                snapshots.push(BranchData::new(*branch_id).load_snapshot(&mut tx).await?);
            }

            snapshots
        } else {
            Vec::new()
        };

        tx.commit_and_then({
            let new_approved = new_approved.clone();
            let event_tx = self.notify().clone();

            move || {
                for snapshot in snapshots {
                    tracing::debug!(
                        branch_id = ?snapshot.branch_id(),
                        hash = ?snapshot.root_hash(),
                        vv = ?snapshot.version_vector(),
                        "snapshot complete"
                    );
                }

                for branch_id in new_approved {
                    event_tx.send(Payload::BranchChanged(branch_id));
                }
            }
        })
        .await?;

        Ok(ReceiveStatus {
            old_approved,
            new_approved,
        })
    }

    async fn check_parent_node_exists(
        &self,
        conn: &mut db::Connection,
        hash: &Hash,
    ) -> Result<(), ReceiveError> {
        if node::parent_exists(conn, hash).await? {
            Ok(())
        } else {
            Err(ReceiveError::ParentNodeNotFound)
        }
    }
}

/// Status of receiving nodes from remote replica.
#[derive(Debug)]
pub(crate) struct ReceiveStatus {
    /// Whether any of the snapshots were already approved.
    pub old_approved: bool,
    /// List of branches whose snapshots have been approved.
    pub new_approved: Vec<PublicKey>,
}

#[derive(Debug, Error)]
pub(crate) enum ReceiveError {
    #[error("proof is invalid")]
    InvalidProof,
    #[error("parent node not found")]
    ParentNodeNotFound,
    #[error("fatal error")]
    Fatal(#[from] Error),
}

impl From<ProofError> for ReceiveError {
    fn from(_: ProofError) -> Self {
        Self::InvalidProof
    }
}

impl From<sqlx::Error> for ReceiveError {
    fn from(error: sqlx::Error) -> Self {
        Self::from(Error::from(error))
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

// Decide what to do with an incoming root node.
struct RootNodeAction {
    // Should we insert the incoming node to the db?
    insert: bool,
    // Should we request the children of the incoming node?
    request_children: bool,
}

async fn decide_root_node_action(
    tx: &mut db::WriteTransaction,
    new_proof: &Proof,
    new_block_presence: &MultiBlockPresence,
) -> Result<RootNodeAction> {
    let mut action = RootNodeAction {
        insert: true,
        request_children: true,
    };

    let mut old_nodes = RootNode::load_all_latest(tx);
    while let Some(old_node) = old_nodes.try_next().await? {
        match new_proof
            .version_vector
            .partial_cmp(&old_node.proof.version_vector)
        {
            Some(Ordering::Less) => {
                // The incoming node is outdated compared to at least one existing node - discard
                // it.
                action.insert = false;
                action.request_children = false;
            }
            Some(Ordering::Equal) => {
                // The incoming node has the same version vector as one of the existing nodes.
                // If the hashes are also equal, there is no point inserting it but if the incoming
                // summary is potentially more up-to-date than the exising one, we still want to
                // request the children. Otherwise we discard it.
                if new_proof.hash == old_node.proof.hash {
                    action.insert = false;

                    // NOTE: `is_outdated` is not antisymmetric, so we can't replace this condition
                    // with `new_summary.is_outdated(&old_node.summary)`.
                    if !old_node
                        .summary
                        .block_presence
                        .is_outdated(new_block_presence)
                    {
                        action.request_children = false;
                    }
                } else {
                    // NOTE: Currently it's possible for two branches to have the same vv but
                    // different hash so we need to accept them.
                    // TODO: When https://github.com/equalitie/ouisync/issues/113 is fixed we can
                    // reject them.
                    tracing::trace!(
                        vv = ?old_node.proof.version_vector,
                        old_hash = ?old_node.proof.hash,
                        new_hash = ?new_proof.hash,
                        "received root node with same vv but different hash"
                    );
                }
            }
            Some(Ordering::Greater) => (),
            None => {
                if new_proof.writer_id == old_node.proof.writer_id {
                    tracing::warn!(
                        old_vv = ?old_node.proof.version_vector,
                        new_vv = ?new_proof.version_vector,
                        writer_id = ?new_proof.writer_id,
                        "received root node invalid: broken invariant - concurrency within branch is not allowed"
                    );

                    action.insert = false;
                    action.request_children = false;
                }
            }
        }

        if !action.insert && !action.request_children {
            break;
        }
    }

    Ok(action)
}

// TODO: move this to some generic utils module.
async fn try_collect_into<S, D, T, E>(src: S, dst: &mut D) -> Result<(), E>
where
    S: Stream<Item = Result<T, E>>,
    D: Extend<T>,
{
    src.try_for_each(|item| {
        dst.extend(iter::once(item));
        future::ready(Ok(()))
    })
    .await
}

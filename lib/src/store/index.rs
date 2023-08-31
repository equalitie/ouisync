//! Operations on the whole index (or its subset) as opposed to individual nodes.

use super::{
    cache::CacheTransaction,
    error::Error,
    inner_node,
    quota::{self, QuotaError},
    receive_filter, root_node,
};
use crate::{
    collections::HashMap,
    crypto::{sign::PublicKey, Hash},
    db,
    future::try_collect_into,
    protocol::NodeState,
    storage_size::StorageSize,
};
use sqlx::Row;

/// Status of receiving nodes from remote replica.
#[derive(Debug)]
pub(super) struct ReceiveStatus {
    /// Whether any of the snapshots were already approved.
    pub old_approved: bool,
    /// List of branches whose snapshots have been approved.
    pub new_approved: Vec<PublicKey>,
}

/// Reason for updating the summary
#[derive(Debug)]
pub(super) enum UpdateSummaryReason {
    /// Updating summary because a block was removed
    BlockRemoved,
    /// Updating summary for some other reason
    Other,
}

/// Does a parent node (root or inner) with the given hash exist?
pub(super) async fn parent_exists(conn: &mut db::Connection, hash: &Hash) -> Result<bool, Error> {
    Ok(sqlx::query(
        "SELECT
             EXISTS(SELECT 0 FROM snapshot_root_nodes  WHERE hash = ?) OR
             EXISTS(SELECT 0 FROM snapshot_inner_nodes WHERE hash = ?)",
    )
    .bind(hash)
    .bind(hash)
    .fetch_one(conn)
    .await?
    .get(0))
}

/// Update summary of the nodes with the specified hashes and all their ancestor nodes.
/// Returns the affected snapshots and their states.
pub(super) async fn update_summaries(
    write_tx: &mut db::WriteTransaction,
    cache_tx: &mut CacheTransaction,
    mut nodes: Vec<Hash>,
    reason: UpdateSummaryReason,
) -> Result<HashMap<Hash, NodeState>, Error> {
    let mut states = HashMap::default();

    while let Some(hash) = nodes.pop() {
        let summary = inner_node::compute_summary(write_tx, &hash).await?;

        // First try inner nodes ...
        let node_infos = inner_node::update_summaries(write_tx, &hash, summary).await?;
        if !node_infos.is_empty() {
            // ... success.

            for (parent_hash, bucket) in node_infos {
                cache_tx.update_inner_summary(parent_hash, bucket, summary);
                nodes.push(parent_hash);
            }

            match reason {
                // If block was removed we need to remove the corresponding receive filter entries
                // so if the block becomes needed again we can request it again.
                UpdateSummaryReason::BlockRemoved => {
                    receive_filter::remove(write_tx, &hash).await?
                }
                UpdateSummaryReason::Other => (),
            }
        } else {
            // ... no hits. Let's try root nodes.
            let state = root_node::update_summaries(write_tx, &hash, summary).await?;
            let summary = summary.with_state(state);

            cache_tx.update_root_summary(hash, summary);

            states
                .entry(hash)
                .or_insert(summary.state)
                .update(summary.state);
        }
    }

    Ok(states)
}

pub(super) async fn finalize(
    write_tx: &mut db::WriteTransaction,
    cache_tx: &mut CacheTransaction,
    hash: Hash,
    quota: Option<StorageSize>,
) -> Result<ReceiveStatus, Error> {
    // TODO: Don't hold write transaction through this whole function. Use it only for
    // `update_summaries` then commit it, then do the quota check with a read-only transaction
    // and then grab another write transaction to do the `approve` / `reject`.
    // CAVEAT: the quota check would need some kind of unique lock to prevent multiple
    // concurrent checks to succeed where they would otherwise fail if ran sequentially.

    let states =
        update_summaries(write_tx, cache_tx, vec![hash], UpdateSummaryReason::Other).await?;

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
            match quota::check(write_tx, &hash, quota).await {
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

            root_node::approve(write_tx, &hash).await?;
            try_collect_into(
                root_node::load_writer_ids(write_tx, &hash),
                &mut new_approved,
            )
            .await?;
        } else {
            root_node::reject(write_tx, &hash).await?;
        }
    }

    Ok(ReceiveStatus {
        old_approved,
        new_approved,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{block, cache::Cache, inner_node, leaf_node, root_node};
    use super::*;
    use crate::{
        crypto::{
            sign::{Keypair, PublicKey},
            Hashable,
        },
        protocol::{
            test_utils::Snapshot, InnerNode, InnerNodeMap, LeafNode, LeafNodeSet,
            MultiBlockPresence, Proof, RootNode, Summary, EMPTY_INNER_HASH, EMPTY_LEAF_HASH,
        },
        test_utils,
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use rand::{rngs::StdRng, SeedableRng};
    use std::iter;
    use std::sync::Arc;
    use tempfile::TempDir;
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_empty_leaf_nodes() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let hash = *EMPTY_LEAF_HASH;
        let summary = inner_node::compute_summary(&mut conn, &hash).await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_incomplete_leaf_nodes() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let node = LeafNode::missing(rand::random(), rand::random());
        let nodes: LeafNodeSet = iter::once(node).collect();
        let hash = nodes.hash();

        let summary = inner_node::compute_summary(&mut conn, &hash).await.unwrap();

        assert_eq!(summary, Summary::INCOMPLETE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_leaf_nodes_with_all_missing_blocks() {
        let (_base_dir, pool) = setup().await;

        let node = LeafNode::missing(rand::random(), rand::random());
        let nodes: LeafNodeSet = iter::once(node).collect();
        let hash = nodes.hash();

        let mut tx = pool.begin_write().await.unwrap();
        leaf_node::save_all(&mut tx, &nodes, &hash).await.unwrap();
        let summary = inner_node::compute_summary(&mut tx, &hash).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_leaf_nodes_with_some_present_blocks() {
        let (_base_dir, pool) = setup().await;

        let node0 = LeafNode::present(rand::random(), rand::random());
        let node1 = LeafNode::missing(rand::random(), rand::random());
        let node2 = LeafNode::missing(rand::random(), rand::random());
        let nodes: LeafNodeSet = vec![node0, node1, node2].into_iter().collect();
        let hash = nodes.hash();

        let mut tx = pool.begin_write().await.unwrap();
        leaf_node::save_all(&mut tx, &nodes, &hash).await.unwrap();
        let summary = inner_node::compute_summary(&mut tx, &hash).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_matches!(summary.block_presence, MultiBlockPresence::Some(_));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_leaf_nodes_with_all_present_blocks() {
        let (_base_dir, pool) = setup().await;

        let node0 = LeafNode::present(rand::random(), rand::random());
        let node1 = LeafNode::present(rand::random(), rand::random());
        let nodes: LeafNodeSet = vec![node0, node1].into_iter().collect();
        let hash = nodes.hash();

        let mut tx = pool.begin_write().await.unwrap();
        leaf_node::save_all(&mut tx, &nodes, &hash).await.unwrap();
        let summary = inner_node::compute_summary(&mut tx, &hash).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::Full);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_empty_inner_nodes() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let hash = *EMPTY_INNER_HASH;
        let summary = inner_node::compute_summary(&mut conn, &hash).await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_incomplete_inner_nodes() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let node = InnerNode::new(rand::random(), Summary::INCOMPLETE);
        let nodes: InnerNodeMap = iter::once((0, node)).collect();
        let hash = nodes.hash();

        let summary = inner_node::compute_summary(&mut conn, &hash).await.unwrap();

        assert_eq!(summary, Summary::INCOMPLETE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_inner_nodes_with_all_missing_blocks() {
        let (_base_dir, pool) = setup().await;

        let inners: InnerNodeMap = (0..2)
            .map(|bucket| {
                let leaf = LeafNode::missing(rand::random(), rand::random());
                let leaf_nodes: LeafNodeSet = iter::once(leaf).collect();

                (
                    bucket,
                    InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes)),
                )
            })
            .collect();

        let hash = inners.hash();

        let mut tx = pool.begin_write().await.unwrap();
        inner_node::save_all(&mut tx, &inners, &hash).await.unwrap();
        let summary = inner_node::compute_summary(&mut tx, &hash).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_inner_nodes_with_some_present_blocks() {
        let (_base_dir, pool) = setup().await;

        // all missing
        let inner0 = {
            let leaf_nodes: LeafNodeSet = (0..2)
                .map(|_| LeafNode::missing(rand::random(), rand::random()))
                .collect();

            InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes))
        };

        // some present
        let inner1 = {
            let leaf_nodes: LeafNodeSet = vec![
                LeafNode::missing(rand::random(), rand::random()),
                LeafNode::present(rand::random(), rand::random()),
            ]
            .into_iter()
            .collect();

            InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes))
        };

        // all present
        let inner2 = {
            let leaf_nodes: LeafNodeSet = (0..2)
                .map(|_| LeafNode::present(rand::random(), rand::random()))
                .collect();

            InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes))
        };

        let inners: InnerNodeMap = vec![(0, inner0), (1, inner1), (2, inner2)]
            .into_iter()
            .collect();
        let hash = inners.hash();

        let mut tx = pool.begin_write().await.unwrap();
        inner_node::save_all(&mut tx, &inners, &hash).await.unwrap();
        let summary = inner_node::compute_summary(&mut tx, &hash).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_matches!(summary.block_presence, MultiBlockPresence::Some(_));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_inner_nodes_with_all_present_blocks() {
        let (_base_dir, pool) = setup().await;

        let inners: InnerNodeMap = (0..2)
            .map(|bucket| {
                let leaf_nodes: LeafNodeSet = (0..2)
                    .map(|_| LeafNode::present(rand::random(), rand::random()))
                    .collect();

                (
                    bucket,
                    InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes)),
                )
            })
            .collect();

        let hash = inners.hash();

        let mut tx = pool.begin_write().await.unwrap();
        inner_node::save_all(&mut tx, &inners, &hash).await.unwrap();
        let summary = inner_node::compute_summary(&mut tx, &hash).await.unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::Full);
    }

    #[proptest]
    fn check_complete(
        #[strategy(0usize..=32)] leaf_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        test_utils::run(check_complete_case(leaf_count, rng_seed))
    }

    async fn check_complete_case(leaf_count: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);

        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let cache = Arc::new(Cache::new());

        let mut write_tx = pool.begin_write().await.unwrap();
        let mut cache_tx = cache.begin();

        let writer_id = PublicKey::generate(&mut rng);
        let write_keys = Keypair::generate(&mut rng);
        let snapshot = Snapshot::generate(&mut rng, leaf_count);

        let mut root_node = root_node::create(
            &mut write_tx,
            Proof::new(
                writer_id,
                VersionVector::first(writer_id),
                *snapshot.root_hash(),
                &write_keys,
            ),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();

        update_summaries_and_approve(&mut write_tx, &mut cache_tx, root_node.proof.hash).await;
        reload_root_node(&mut write_tx, &mut root_node)
            .await
            .unwrap();
        assert_eq!(root_node.summary.state.is_approved(), leaf_count == 0);

        // TODO: consider randomizing the order the nodes are saved so it's not always
        // breadth-first.

        for layer in snapshot.inner_layers() {
            for (parent_hash, nodes) in layer.inner_maps() {
                inner_node::save_all(&mut write_tx, &nodes.clone().into_incomplete(), parent_hash)
                    .await
                    .unwrap();

                update_summaries_and_approve(&mut write_tx, &mut cache_tx, *parent_hash).await;

                reload_root_node(&mut write_tx, &mut root_node)
                    .await
                    .unwrap();
                assert!(!root_node.summary.state.is_approved());
            }
        }

        let mut unsaved_leaves = snapshot.leaf_count();

        for (parent_hash, nodes) in snapshot.leaf_sets() {
            leaf_node::save_all(&mut write_tx, &nodes.clone().into_missing(), parent_hash)
                .await
                .unwrap();
            unsaved_leaves -= nodes.len();

            update_summaries_and_approve(&mut write_tx, &mut cache_tx, *parent_hash).await;
            reload_root_node(&mut write_tx, &mut root_node)
                .await
                .unwrap();

            if unsaved_leaves > 0 {
                assert!(!root_node.summary.state.is_approved());
            }
        }

        assert!(root_node.summary.state.is_approved());

        // HACK: prevent "too many open files" error.
        drop(write_tx);
        pool.close().await.unwrap();

        async fn update_summaries_and_approve(
            write_tx: &mut db::WriteTransaction,
            cache_tx: &mut CacheTransaction,
            hash: Hash,
        ) {
            for (hash, state) in
                update_summaries(write_tx, cache_tx, vec![hash], UpdateSummaryReason::Other)
                    .await
                    .unwrap()
            {
                match state {
                    NodeState::Complete => {
                        root_node::approve(write_tx, &hash).await.unwrap();
                    }
                    NodeState::Incomplete | NodeState::Approved => (),
                    NodeState::Rejected => unreachable!(),
                }
            }
        }
    }

    #[proptest]
    fn summary(
        #[strategy(0usize..=32)] leaf_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        test_utils::run(summary_case(leaf_count, rng_seed))
    }

    async fn summary_case(leaf_count: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let cache = Arc::new(Cache::new());

        let mut write_tx = pool.begin_write().await.unwrap();
        let mut cache_tx = cache.begin();

        let writer_id = PublicKey::generate(&mut rng);
        let write_keys = Keypair::generate(&mut rng);
        let snapshot = Snapshot::generate(&mut rng, leaf_count);

        // Save the snapshot initially with all nodes missing.
        let mut root_node = root_node::create(
            &mut write_tx,
            Proof::new(
                writer_id,
                VersionVector::first(writer_id),
                *snapshot.root_hash(),
                &write_keys,
            ),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();

        if snapshot.leaf_count() == 0 {
            update_summaries(
                &mut write_tx,
                &mut cache_tx,
                vec![root_node.proof.hash],
                UpdateSummaryReason::Other,
            )
            .await
            .unwrap();
        }

        for layer in snapshot.inner_layers() {
            for (parent_hash, nodes) in layer.inner_maps() {
                inner_node::save_all(&mut write_tx, &nodes.clone().into_incomplete(), parent_hash)
                    .await
                    .unwrap();
            }
        }

        for (parent_hash, nodes) in snapshot.leaf_sets() {
            leaf_node::save_all(&mut write_tx, &nodes.clone().into_missing(), parent_hash)
                .await
                .unwrap();
            update_summaries(
                &mut write_tx,
                &mut cache_tx,
                vec![*parent_hash],
                UpdateSummaryReason::Other,
            )
            .await
            .unwrap();
        }

        // Check that initially all blocks are missing
        reload_root_node(&mut write_tx, &mut root_node)
            .await
            .unwrap();

        assert_eq!(root_node.summary.block_presence, MultiBlockPresence::None);

        let mut received_blocks = 0;

        for block in snapshot.blocks().values() {
            block::receive(&mut write_tx, &mut cache_tx, block)
                .await
                .unwrap();
            received_blocks += 1;

            reload_root_node(&mut write_tx, &mut root_node)
                .await
                .unwrap();

            if received_blocks < snapshot.blocks().len() {
                assert_matches!(
                    root_node.summary.block_presence,
                    MultiBlockPresence::Some(_)
                );
            } else {
                assert_eq!(root_node.summary.block_presence, MultiBlockPresence::Full);
            }

            // TODO: check also inner and leaf nodes
        }

        // HACK: prevent "too many open files" error.
        drop(write_tx);
        pool.close().await.unwrap();
    }

    async fn setup() -> (TempDir, db::Pool) {
        db::create_temp().await.unwrap()
    }

    // Reload this root node from the db.
    #[cfg(test)]
    async fn reload_root_node(conn: &mut db::Connection, node: &mut RootNode) -> Result<(), Error> {
        let row = sqlx::query(
            "SELECT state, block_presence
             FROM snapshot_root_nodes
             WHERE snapshot_id = ?",
        )
        .bind(node.snapshot_id)
        .fetch_one(conn)
        .await?;

        node.summary.state = row.get(0);
        node.summary.block_presence = row.get(1);

        Ok(())
    }
}

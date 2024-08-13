//! Operations on the whole index (or its subset) as opposed to individual nodes.

use super::{cache::CacheTransaction, error::Error, inner_node, root_node};
use crate::{collections::HashMap, crypto::Hash, db, protocol::NodeState};
use sqlx::Row;

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
        } else {
            // ... no hits. Let's try root nodes.
            let state = root_node::update_summaries(write_tx, &hash, summary).await?;
            let summary = summary.with_state(state);

            cache_tx.update_root_summary(hash, summary);
            states.insert(hash, summary.state);
        }
    }

    Ok(states)
}

#[cfg(test)]
mod tests {
    use super::super::{cache::Cache, inner_node, leaf_node, root_node};
    use super::*;
    use crate::future::TryStreamExt as _;
    use crate::store::block;
    use crate::{
        crypto::{
            sign::{Keypair, PublicKey},
            Hashable,
        },
        protocol::{
            test_utils::Snapshot, InnerNode, InnerNodes, LeafNode, LeafNodes, MultiBlockPresence,
            Proof, RootNode, RootNodeFilter, Summary, EMPTY_INNER_HASH, EMPTY_LEAF_HASH,
        },
        test_utils,
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use futures_util::TryStreamExt;
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
        let nodes: LeafNodes = iter::once(node).collect();
        let hash = nodes.hash();

        let summary = inner_node::compute_summary(&mut conn, &hash).await.unwrap();

        assert_eq!(summary, Summary::INCOMPLETE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_leaf_nodes_with_all_missing_blocks() {
        let (_base_dir, pool) = setup().await;

        let node = LeafNode::missing(rand::random(), rand::random());
        let nodes: LeafNodes = iter::once(node).collect();
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
        let nodes: LeafNodes = vec![node0, node1, node2].into_iter().collect();
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
        let nodes: LeafNodes = vec![node0, node1].into_iter().collect();
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
        let nodes: InnerNodes = iter::once((0, node)).collect();
        let hash = nodes.hash();

        let summary = inner_node::compute_summary(&mut conn, &hash).await.unwrap();

        assert_eq!(summary, Summary::INCOMPLETE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_inner_nodes_with_all_missing_blocks() {
        let (_base_dir, pool) = setup().await;

        let inners: InnerNodes = (0..2)
            .map(|bucket| {
                let leaf = LeafNode::missing(rand::random(), rand::random());
                let leaf_nodes: LeafNodes = iter::once(leaf).collect();

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
            let leaf_nodes: LeafNodes = (0..2)
                .map(|_| LeafNode::missing(rand::random(), rand::random()))
                .collect();

            InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes))
        };

        // some present
        let inner1 = {
            let leaf_nodes: LeafNodes = vec![
                LeafNode::missing(rand::random(), rand::random()),
                LeafNode::present(rand::random(), rand::random()),
            ]
            .into_iter()
            .collect();

            InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes))
        };

        // all present
        let inner2 = {
            let leaf_nodes: LeafNodes = (0..2)
                .map(|_| LeafNode::present(rand::random(), rand::random()))
                .collect();

            InnerNode::new(leaf_nodes.hash(), Summary::from_leaves(&leaf_nodes))
        };

        let inners: InnerNodes = vec![(0, inner0), (1, inner1), (2, inner2)]
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

        let inners: InnerNodes = (0..2)
            .map(|bucket| {
                let leaf_nodes: LeafNodes = (0..2)
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

        let (mut root_node, _) = root_node::create(
            &mut write_tx,
            Proof::new(
                writer_id,
                VersionVector::first(writer_id),
                *snapshot.root_hash(),
                &write_keys,
            ),
            Summary::INCOMPLETE,
            RootNodeFilter::Published,
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
            for (hash, state) in update_summaries(write_tx, cache_tx, vec![hash])
                .await
                .unwrap()
            {
                match state {
                    NodeState::Complete => {
                        root_node::approve(write_tx, &hash)
                            .try_consume()
                            .await
                            .unwrap();
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
        let (mut root_node, _) = root_node::create(
            &mut write_tx,
            Proof::new(
                writer_id,
                VersionVector::first(writer_id),
                *snapshot.root_hash(),
                &write_keys,
            ),
            Summary::INCOMPLETE,
            RootNodeFilter::Any,
        )
        .await
        .unwrap();

        if snapshot.leaf_count() == 0 {
            update_summaries(&mut write_tx, &mut cache_tx, vec![root_node.proof.hash])
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
            update_summaries(&mut write_tx, &mut cache_tx, vec![*parent_hash])
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
            block::write(&mut write_tx, block).await.unwrap();
            let parent_hashes = leaf_node::set_present(&mut write_tx, &block.id)
                .try_collect()
                .await
                .unwrap();
            update_summaries(&mut write_tx, &mut cache_tx, parent_hashes)
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

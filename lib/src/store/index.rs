//! Operations on the whole index (or its subset) as opposed to individual nodes.

use std::collections::{HashMap, HashSet};

use super::{error::Error, inner_node, root_node};
use crate::{
    crypto::Hash,
    db::{self, MAX_BIND_PARAMS},
    future::TryStreamExt as _,
    protocol::NodeState,
};
use futures_util::TryStreamExt as _;
use sqlx::{QueryBuilder, Row};

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

/// Filter node hashes corresponding to existing root or inner nodes.
pub(super) async fn filter_existing_parents(
    conn: &mut db::Connection,
    hashes: &[Hash],
) -> Result<HashSet<Hash>, Error> {
    let mut output = HashSet::with_capacity(hashes.len());

    for hash_chunk in hashes.chunks(MAX_BIND_PARAMS) {
        let mut builder = QueryBuilder::new(
            "SELECT hash FROM (
                 SELECT hash FROM snapshot_root_nodes
                 UNION ALL
                 SELECT hash FROM snapshot_inner_nodes
             ) WHERE hash in (",
        );

        let mut separated = builder.separated(", ");
        for hash in hash_chunk {
            separated.push_bind(hash);
        }
        builder.push(")");
        builder
            .build()
            .persistent(false)
            .fetch(&mut *conn)
            .map_ok(|row| row.get::<Hash, _>(0))
            .try_collect_into(&mut output)
            .await?;
    }

    Ok(output)
}

/// Update summary of the nodes with the specified hashes and all their ancestor nodes.
/// Returns the affected snapshots and their states.
pub(super) async fn update_summaries(
    tx: &mut db::WriteTransaction,
    nodes: &[Hash],
) -> Result<HashMap<Hash, NodeState>, Error> {
    let mut states = HashMap::new();

    let mut curr = nodes;
    let mut next = Vec::new();

    while !curr.is_empty() {
        let summaries = inner_node::compute_summaries(tx, curr).await?;
        let updated = inner_node::update_summaries(tx, &summaries).await?;

        // Handle root node summaries
        for (hash, summary) in summaries {
            if updated.contains_key(&hash) {
                continue;
            }

            let state = root_node::update_summaries(tx, &hash, summary).await?;
            let summary = summary.with_state(state);
            states.insert(hash, summary.state);
        }

        next.clear();
        next.extend(updated.into_values());
        curr = &next;
    }

    Ok(states)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{block::BlockWriter, inner_node, leaf_node, root_node},
        *,
    };
    use crate::{
        crypto::{
            Hashable,
            sign::{Keypair, PublicKey},
        },
        protocol::{
            EMPTY_INNER_HASH, EMPTY_LEAF_HASH, InnerNode, InnerNodes, LeafNode, LeafNodes,
            MultiBlockPresence, Proof, RootNode, RootNodeFilter, Summary, test_utils::Snapshot,
        },
        test_utils,
        version_vector::VersionVector,
    };
    use assert_matches::assert_matches;
    use rand::{SeedableRng, rngs::StdRng};
    use std::{iter, slice};
    use tempfile::TempDir;
    use test_strategy::proptest;

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_empty_leaf_nodes() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let hash = *EMPTY_LEAF_HASH;
        let (_, summary) = inner_node::compute_summaries(&mut conn, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

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

        let (_, summary) = inner_node::compute_summaries(&mut conn, &[hash])
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

        assert_eq!(summary, Summary::INCOMPLETE);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_complete_leaf_nodes_with_all_missing_blocks() {
        let (_base_dir, pool) = setup().await;

        let node = LeafNode::missing(rand::random(), rand::random());
        let nodes: LeafNodes = iter::once(node).collect();
        let hash = nodes.hash();

        let mut tx = pool.begin_write().await.unwrap();
        leaf_node::save(
            &mut tx,
            &nodes.iter().map(|node| (hash, *node)).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let (_, summary) = inner_node::compute_summaries(&mut tx, &[hash])
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
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
        leaf_node::save(
            &mut tx,
            &nodes.iter().map(|node| (hash, *node)).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let (_, summary) = inner_node::compute_summaries(&mut tx, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
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
        leaf_node::save(
            &mut tx,
            &nodes.iter().map(|node| (hash, *node)).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let (_, summary) = inner_node::compute_summaries(&mut tx, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::Full);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compute_summary_from_empty_inner_nodes() {
        let (_base_dir, pool) = setup().await;
        let mut conn = pool.acquire().await.unwrap();

        let hash = *EMPTY_INNER_HASH;
        let (_, summary) = inner_node::compute_summaries(&mut conn, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

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

        let (_, summary) = inner_node::compute_summaries(&mut conn, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();

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
        inner_node::save(
            &mut tx,
            &inners
                .iter()
                .map(|(bucket, node)| (hash, bucket, *node))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let (_, summary) = inner_node::compute_summaries(&mut tx, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
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
        inner_node::save(
            &mut tx,
            &inners
                .iter()
                .map(|(bucket, node)| (hash, bucket, *node))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let (_, summary) = inner_node::compute_summaries(&mut tx, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
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
        inner_node::save(
            &mut tx,
            &inners
                .iter()
                .map(|(bucket, node)| (hash, bucket, *node))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let (_, summary) = inner_node::compute_summaries(&mut tx, slice::from_ref(&hash))
            .await
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(summary.state, NodeState::Complete);
        assert_eq!(summary.block_presence, MultiBlockPresence::Full);
    }

    #[proptest(async = "tokio")]
    async fn check_complete(
        #[strategy(0usize..=32)] leaf_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        check_complete_case(leaf_count, rng_seed).await
    }

    async fn check_complete_case(leaf_count: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);

        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let mut tx = pool.begin_write().await.unwrap();

        let writer_id = PublicKey::generate(&mut rng);
        let write_keys = Keypair::generate(&mut rng);
        let snapshot = Snapshot::generate(&mut rng, leaf_count);

        let (mut root_node, _) = root_node::create(
            &mut tx,
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

        update_summaries_and_approve(&mut tx, root_node.proof.hash).await;
        reload_root_node(&mut tx, &mut root_node).await.unwrap();
        assert_eq!(root_node.summary.state.is_approved(), leaf_count == 0);

        // TODO: consider randomizing the order the nodes are saved so it's not always
        // breadth-first.

        for (parent_hash, nodes) in snapshot.inner_sets() {
            inner_node::save(
                &mut tx,
                &nodes
                    .iter()
                    .map(|(bucket, node)| (*parent_hash, bucket, node.into_incomplete()))
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();

            update_summaries_and_approve(&mut tx, *parent_hash).await;

            reload_root_node(&mut tx, &mut root_node).await.unwrap();
            assert!(!root_node.summary.state.is_approved());
        }

        let mut unsaved_leaves = snapshot.leaf_count();

        for (parent_hash, nodes) in snapshot.leaf_sets() {
            leaf_node::save(
                &mut tx,
                &nodes
                    .iter()
                    .map(|node| (*parent_hash, node.into_missing()))
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
            unsaved_leaves -= nodes.len();

            update_summaries_and_approve(&mut tx, *parent_hash).await;
            reload_root_node(&mut tx, &mut root_node).await.unwrap();

            if unsaved_leaves > 0 {
                assert!(!root_node.summary.state.is_approved());
            }
        }

        assert!(root_node.summary.state.is_approved());

        // HACK: prevent "too many open files" error.
        drop(tx);
        pool.close().await.unwrap();

        async fn update_summaries_and_approve(tx: &mut db::WriteTransaction, hash: Hash) {
            for (hash, state) in update_summaries(tx, slice::from_ref(&hash)).await.unwrap() {
                match state {
                    NodeState::Complete => {
                        root_node::approve(tx, &hash).try_consume().await.unwrap();
                    }
                    NodeState::Incomplete | NodeState::Approved => (),
                    NodeState::Rejected => unreachable!(),
                }
            }
        }
    }

    #[proptest(async = "tokio")]
    async fn summary(
        #[strategy(0usize..=32)] leaf_count: usize,
        #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
    ) {
        summary_case(leaf_count, rng_seed).await
    }

    async fn summary_case(leaf_count: usize, rng_seed: u64) {
        let mut rng = StdRng::seed_from_u64(rng_seed);
        let (_base_dir, pool) = db::create_temp().await.unwrap();
        let mut tx = pool.begin_write().await.unwrap();

        let writer_id = PublicKey::generate(&mut rng);
        let write_keys = Keypair::generate(&mut rng);
        let snapshot = Snapshot::generate(&mut rng, leaf_count);

        // Save the snapshot initially with all nodes missing.
        let (mut root_node, _) = root_node::create(
            &mut tx,
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
            update_summaries(&mut tx, slice::from_ref(&root_node.proof.hash))
                .await
                .unwrap();
        }

        for (parent_hash, nodes) in snapshot.inner_sets() {
            inner_node::save(
                &mut tx,
                &nodes
                    .iter()
                    .map(|(bucket, node)| (*parent_hash, bucket, node.into_incomplete()))
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
        }

        for (parent_hash, nodes) in snapshot.leaf_sets() {
            leaf_node::save(
                &mut tx,
                &nodes
                    .iter()
                    .map(|node| (*parent_hash, node.into_missing()))
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
            update_summaries(&mut tx, slice::from_ref(parent_hash))
                .await
                .unwrap();
        }

        // Check that initially all blocks are missing
        reload_root_node(&mut tx, &mut root_node).await.unwrap();

        assert_eq!(root_node.summary.block_presence, MultiBlockPresence::None);

        let mut received_blocks = 0;

        for block in snapshot.blocks().values() {
            BlockWriter::new(&mut tx)
                .add(block)
                .await
                .unwrap()
                .finish()
                .await
                .unwrap();
            let parent_hashes: Vec<_> = leaf_node::set_present(&mut tx, slice::from_ref(&block.id))
                .await
                .unwrap()
                .into_values()
                .flatten()
                .map(|update| update.parent)
                .collect();
            update_summaries(&mut tx, &parent_hashes).await.unwrap();

            received_blocks += 1;

            reload_root_node(&mut tx, &mut root_node).await.unwrap();

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
        drop(tx);
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

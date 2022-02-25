use super::{
    super::proof::Proof, leaf::EMPTY_LEAF_HASH, summary::Summary, test_utils::Snapshot, *,
};
use crate::{
    crypto::{
        sign::{Keypair, PublicKey},
        Hashable,
    },
    db,
    error::Error,
    test_utils,
    version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::TryStreamExt;
use rand::prelude::*;
use std::iter;
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn create_new_root_node() {
    let mut conn = setup().await;

    let writer_id = PublicKey::random();
    let write_keys = Keypair::random();
    let hash = rand::random();

    let node0 = RootNode::create(
        &mut conn,
        Proof::new(writer_id, VersionVector::new(), hash, &write_keys),
        Summary::FULL,
    )
    .await
    .unwrap();
    assert_eq!(node0.proof.hash, hash);

    let node1 = RootNode::load_latest_by_writer(&mut conn, writer_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(node1, node0);

    let nodes: Vec<_> = RootNode::load_all_by_writer(&mut conn, writer_id, 2)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0], node0);
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_create_existing_root_node() {
    let mut conn = setup().await;

    let writer_id = PublicKey::random();
    let write_keys = Keypair::random();
    let hash = rand::random();

    let node = RootNode::create(
        &mut conn,
        Proof::new(writer_id, VersionVector::new(), hash, &write_keys),
        Summary::FULL,
    )
    .await
    .unwrap();

    assert_matches!(
        RootNode::create(
            &mut conn,
            Proof::new(writer_id, VersionVector::new(), hash, &write_keys),
            Summary::FULL,
        )
        .await,
        Err(Error::EntryExists)
    );

    let nodes: Vec<_> = RootNode::load_all_by_writer(&mut conn, writer_id, 2)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0], node);
}

#[tokio::test(flavor = "multi_thread")]
async fn create_new_inner_node() {
    let mut conn = setup().await;

    let parent = rand::random();
    let hash = rand::random();
    let bucket = rand::random();

    let mut tx = conn.begin().await.unwrap();

    let node = InnerNode::new(hash, Summary::FULL);
    node.save(&mut tx, &parent, bucket).await.unwrap();

    let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();

    assert_eq!(nodes.get(bucket), Some(&node));

    assert!((0..bucket).all(|b| nodes.get(b).is_none()));

    if bucket < u8::MAX {
        assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_existing_inner_node() {
    let mut conn = setup().await;

    let parent = rand::random();
    let hash = rand::random();
    let bucket = rand::random();

    let mut tx = conn.begin().await.unwrap();

    let node0 = InnerNode::new(hash, Summary::FULL);
    node0.save(&mut tx, &parent, bucket).await.unwrap();

    let node1 = InnerNode::new(hash, Summary::FULL);
    node1.save(&mut tx, &parent, bucket).await.unwrap();

    let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();

    assert_eq!(nodes.get(bucket), Some(&node0));
    assert!((0..bucket).all(|b| nodes.get(b).is_none()));

    if bucket < u8::MAX {
        assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_create_conflicting_inner_node() {
    let mut conn = setup().await;

    let parent = rand::random();
    let bucket = rand::random();

    let hash0 = rand::random();
    let hash1 = loop {
        let hash = rand::random();
        if hash != hash0 {
            break hash;
        }
    };

    let mut tx = conn.begin().await.unwrap();

    let node0 = InnerNode::new(hash0, Summary::FULL);
    node0.save(&mut tx, &parent, bucket).await.unwrap();

    let node1 = InnerNode::new(hash1, Summary::FULL);
    assert_matches!(node1.save(&mut tx, &parent, bucket).await, Err(_)); // TODO: match concrete error type
}

#[tokio::test(flavor = "multi_thread")]
async fn save_new_present_leaf_node() {
    let mut conn = setup().await;

    let parent = rand::random();
    let encoded_locator = rand::random();
    let block_id = rand::random();

    let node = LeafNode::present(encoded_locator, block_id);
    node.save(&mut conn, &parent).await.unwrap();

    let nodes = LeafNode::load_children(&mut conn, &parent).await.unwrap();
    assert_eq!(nodes.len(), 1);

    let node = nodes.get(&encoded_locator).unwrap();
    assert_eq!(node.locator(), &encoded_locator);
    assert_eq!(node.block_id, block_id);
    assert!(!node.is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn save_new_missing_leaf_node() {
    let mut conn = setup().await;

    let parent = rand::random();
    let encoded_locator = rand::random();
    let block_id = rand::random();

    let mut tx = conn.begin().await.unwrap();

    let node = LeafNode::missing(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    let nodes = LeafNode::load_children(&mut tx, &parent).await.unwrap();
    assert_eq!(nodes.len(), 1);

    let node = nodes.get(&encoded_locator).unwrap();
    assert_eq!(node.locator(), &encoded_locator);
    assert_eq!(node.block_id, block_id);
    assert!(node.is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn save_missing_leaf_node_over_existing_missing_one() {
    let mut conn = setup().await;

    let parent = rand::random();
    let encoded_locator = rand::random();
    let block_id = rand::random();

    let mut tx = conn.begin().await.unwrap();

    let node = LeafNode::missing(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    let node = LeafNode::missing(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    let nodes = LeafNode::load_children(&mut tx, &parent).await.unwrap();
    assert_eq!(nodes.len(), 1);

    let node = nodes.get(&encoded_locator).unwrap();
    assert_eq!(node.locator(), &encoded_locator);
    assert_eq!(node.block_id, block_id);
    assert!(node.is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn save_missing_leaf_node_over_existing_present_one() {
    let mut conn = setup().await;

    let parent = rand::random();
    let encoded_locator = rand::random();
    let block_id = rand::random();

    let node = LeafNode::present(encoded_locator, block_id);
    node.save(&mut conn, &parent).await.unwrap();

    let node = LeafNode::missing(encoded_locator, block_id);
    node.save(&mut conn, &parent).await.unwrap();

    let nodes = LeafNode::load_children(&mut conn, &parent).await.unwrap();
    assert_eq!(nodes.len(), 1);

    let node = nodes.get(&encoded_locator).unwrap();
    assert_eq!(node.locator(), &encoded_locator);
    assert_eq!(node.block_id, block_id);
    assert!(!node.is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_empty_leaf_nodes() {
    let mut conn = setup().await;

    let hash = *EMPTY_LEAF_HASH;
    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_incomplete_leaf_nodes() {
    let mut conn = setup().await;

    let node = LeafNode::missing(rand::random(), rand::random());
    let nodes: LeafNodeSet = iter::once(node).collect();
    let hash = nodes.hash();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert_eq!(summary, Summary::INCOMPLETE);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_leaf_nodes_with_all_missing_blocks() {
    let mut conn = setup().await;

    let node = LeafNode::missing(rand::random(), rand::random());
    let nodes: LeafNodeSet = iter::once(node).collect();
    let hash = nodes.hash();
    nodes.save(&mut conn, &hash).await.unwrap();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_leaf_nodes_with_some_present_blocks() {
    let mut conn = setup().await;

    let node0 = LeafNode::present(rand::random(), rand::random());
    let node1 = LeafNode::missing(rand::random(), rand::random());
    let node2 = LeafNode::missing(rand::random(), rand::random());
    let nodes: LeafNodeSet = vec![node0, node1, node2].into_iter().collect();
    let hash = nodes.hash();
    nodes.save(&mut conn, &hash).await.unwrap();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_leaf_nodes_with_all_present_blocks() {
    let mut conn = setup().await;

    let node0 = LeafNode::present(rand::random(), rand::random());
    let node1 = LeafNode::present(rand::random(), rand::random());
    let nodes: LeafNodeSet = vec![node0, node1].into_iter().collect();
    let hash = nodes.hash();
    nodes.save(&mut conn, &hash).await.unwrap();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_empty_inner_nodes() {
    let mut conn = setup().await;

    let hash = *EMPTY_INNER_HASH;
    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_incomplete_inner_nodes() {
    let mut conn = setup().await;

    let node = InnerNode::new(rand::random(), Summary::INCOMPLETE);
    let nodes: InnerNodeMap = iter::once((0, node)).collect();
    let hash = nodes.hash();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert_eq!(summary, Summary::INCOMPLETE);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_inner_nodes_with_all_missing_blocks() {
    let mut conn = setup().await;

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
    inners.save(&mut conn, &hash).await.unwrap();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_inner_nodes_with_some_present_blocks() {
    let mut conn = setup().await;

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
    inners.save(&mut conn, &hash).await.unwrap();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_inner_nodes_with_all_present_blocks() {
    let mut conn = setup().await;

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
    inners.save(&mut conn, &hash).await.unwrap();

    let summary = InnerNode::compute_summary(&mut conn, &hash).await.unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_present_on_leaf_node_with_missing_block() {
    let mut conn = setup().await;

    let parent = rand::random();
    let encoded_locator = rand::random();
    let block_id = rand::random();

    let node = LeafNode::missing(encoded_locator, block_id);
    node.save(&mut conn, &parent).await.unwrap();

    assert!(LeafNode::set_present(&mut conn, &block_id).await.unwrap());

    let nodes = LeafNode::load_children(&mut conn, &parent).await.unwrap();
    assert!(!nodes.get(&encoded_locator).unwrap().is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_present_on_leaf_node_with_present_block() {
    let mut conn = setup().await;

    let parent = rand::random();
    let encoded_locator = rand::random();
    let block_id = rand::random();

    let node = LeafNode::present(encoded_locator, block_id);
    node.save(&mut conn, &parent).await.unwrap();

    assert!(!LeafNode::set_present(&mut conn, &block_id).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn set_present_on_leaf_node_that_does_not_exist() {
    let mut conn = setup().await;

    let block_id = rand::random();

    assert_matches!(
        LeafNode::set_present(&mut conn, &block_id).await,
        Err(Error::BlockNotReferenced)
    )
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
    let mut conn = setup().await;

    let writer_id = PublicKey::generate(&mut rng);
    let write_keys = Keypair::generate(&mut rng);
    let snapshot = Snapshot::generate(&mut rng, leaf_count);

    let mut root_node = RootNode::create(
        &mut conn,
        Proof::new(
            writer_id,
            VersionVector::new(),
            *snapshot.root_hash(),
            &write_keys,
        ),
        Summary::FULL,
    )
    .await
    .unwrap();

    super::update_summaries(&mut conn, root_node.proof.hash)
        .await
        .unwrap();
    root_node.reload(&mut conn).await.unwrap();
    assert_eq!(root_node.summary.is_complete(), leaf_count == 0);

    // TODO: consider randomizing the order the nodes are saved so it's not always
    // breadth-first.

    for layer in snapshot.inner_layers() {
        for (parent_hash, nodes) in layer.inner_maps() {
            nodes.save(&mut conn, parent_hash).await.unwrap();
            super::update_summaries(&mut conn, *parent_hash)
                .await
                .unwrap();
            root_node.reload(&mut conn).await.unwrap();
            assert!(!root_node.summary.is_complete());
        }
    }

    let mut unsaved_leaves = snapshot.leaf_count();

    for (parent_hash, nodes) in snapshot.leaf_sets() {
        nodes.save(&mut conn, parent_hash).await.unwrap();
        unsaved_leaves -= nodes.len();

        super::update_summaries(&mut conn, *parent_hash)
            .await
            .unwrap();
        root_node.reload(&mut conn).await.unwrap();

        if unsaved_leaves > 0 {
            assert!(!root_node.summary.is_complete());
        }
    }

    assert!(root_node.summary.is_complete());
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
    let mut conn = setup().await;

    let writer_id = PublicKey::generate(&mut rng);
    let write_keys = Keypair::generate(&mut rng);
    let snapshot = Snapshot::generate(&mut rng, leaf_count);

    // Save the snapshot initially with all nodes missing.
    let mut root_node = RootNode::create(
        &mut conn,
        Proof::new(
            writer_id,
            VersionVector::new(),
            *snapshot.root_hash(),
            &write_keys,
        ),
        Summary::INCOMPLETE,
    )
    .await
    .unwrap();

    if snapshot.leaf_count() == 0 {
        super::update_summaries(&mut conn, root_node.proof.hash)
            .await
            .unwrap();
    }

    for layer in snapshot.inner_layers() {
        for (parent_hash, nodes) in layer.inner_maps() {
            nodes
                .clone()
                .into_incomplete()
                .save(&mut conn, parent_hash)
                .await
                .unwrap();
        }
    }

    for (parent_hash, nodes) in snapshot.leaf_sets() {
        nodes
            .clone()
            .into_missing()
            .save(&mut conn, parent_hash)
            .await
            .unwrap();

        super::update_summaries(&mut conn, *parent_hash)
            .await
            .unwrap();
    }

    let mut expected_missing_blocks_count = snapshot.leaf_count() as u64;

    // Check that initially all blocks are missing
    root_node.reload(&mut conn).await.unwrap();

    assert_eq!(
        root_node.summary.missing_blocks_count,
        expected_missing_blocks_count
    );

    // Keep receiving the blocks one by one and verify the missing blocks summaries get updated
    // accordingly.
    for block_id in snapshot.blocks().keys() {
        super::receive_block(&mut conn, block_id).await.unwrap();

        expected_missing_blocks_count -= 1;

        root_node.reload(&mut conn).await.unwrap();
        assert_eq!(
            root_node.summary.missing_blocks_count,
            expected_missing_blocks_count
        );

        // TODO: check also inner and leaf nodes
    }
}

async fn setup() -> db::Connection {
    let mut conn = db::open_or_create(&db::Store::Temporary)
        .await
        .unwrap()
        .acquire()
        .await
        .unwrap()
        .detach();
    super::super::init(&mut conn).await.unwrap();
    conn
}
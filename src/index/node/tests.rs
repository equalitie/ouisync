use std::iter;

use super::{inner::INNER_LAYER_COUNT, summary::Summary, test_utils::Snapshot, *};
use crate::{crypto::Hashable, db, error::Error, test_utils, version_vector::VersionVector};
use assert_matches::assert_matches;
use futures_util::TryStreamExt;
use rand::prelude::*;
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn create_new_root_node() {
    let pool = setup().await;

    let replica_id = rand::random();
    let hash = rand::random::<u64>().hash();

    let node0 = RootNode::create(
        &pool,
        &replica_id,
        VersionVector::new(),
        hash,
        Summary::FULL,
    )
    .await
    .unwrap();
    assert_eq!(node0.hash, hash);

    let node1 = RootNode::load_latest_or_create(&pool, &replica_id)
        .await
        .unwrap();
    assert_eq!(node1, node0);

    let nodes: Vec<_> = RootNode::load_all(&pool, &replica_id, 2)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0], node0);
}

#[tokio::test(flavor = "multi_thread")]
async fn create_existing_root_node() {
    let pool = setup().await;

    let replica_id = rand::random();
    let hash = rand::random::<u64>().hash();

    let node0 = RootNode::create(
        &pool,
        &replica_id,
        VersionVector::new(),
        hash,
        Summary::FULL,
    )
    .await
    .unwrap();

    let node1 = RootNode::create(
        &pool,
        &replica_id,
        VersionVector::new(),
        hash,
        Summary::FULL,
    )
    .await
    .unwrap();
    assert_eq!(node0, node1);

    let nodes: Vec<_> = RootNode::load_all(&pool, &replica_id, 2)
        .try_collect()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0], node0);
}

#[tokio::test(flavor = "multi_thread")]
async fn create_new_inner_node() {
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let hash = rand::random::<u64>().hash();
    let bucket = rand::random();

    let mut tx = pool.begin().await.unwrap();

    let node = InnerNode::new(hash);
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
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let hash = rand::random::<u64>().hash();
    let bucket = rand::random();

    let mut tx = pool.begin().await.unwrap();

    let node0 = InnerNode::new(hash);
    node0.save(&mut tx, &parent, bucket).await.unwrap();

    let node1 = InnerNode::new(hash);
    node1.save(&mut tx, &parent, bucket).await.unwrap();

    let nodes = InnerNode::load_children(&mut tx, &parent).await.unwrap();

    assert_eq!(nodes.get(bucket), Some(&node0));
    assert!((0..bucket).all(|b| nodes.get(b).is_none()));
    assert!((bucket + 1..=u8::MAX).all(|b| nodes.get(b).is_none()));
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_create_conflicting_inner_node() {
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let bucket = rand::random();

    let hash0 = rand::random::<u64>().hash();
    let hash1 = loop {
        let hash = rand::random::<u64>().hash();
        if hash != hash0 {
            break hash;
        }
    };

    let mut tx = pool.begin().await.unwrap();

    let node0 = InnerNode::new(hash0);
    node0.save(&mut tx, &parent, bucket).await.unwrap();

    let node1 = InnerNode::new(hash1);
    assert_matches!(node1.save(&mut tx, &parent, bucket).await, Err(_)); // TODO: match concrete error type
}

#[tokio::test(flavor = "multi_thread")]
async fn save_new_present_leaf_node() {
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let encoded_locator = rand::random::<u64>().hash();
    let block_id = rand::random();

    let mut tx = pool.begin().await.unwrap();

    let node = LeafNode::present(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    let nodes = LeafNode::load_children(&mut tx, &parent).await.unwrap();
    assert_eq!(nodes.len(), 1);

    let node = nodes.get(&encoded_locator).unwrap();
    assert_eq!(node.locator(), &encoded_locator);
    assert_eq!(node.block_id, block_id);
    assert!(!node.is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn save_new_missing_leaf_node() {
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let encoded_locator = rand::random::<u64>().hash();
    let block_id = rand::random();

    let mut tx = pool.begin().await.unwrap();

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
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let encoded_locator = rand::random::<u64>().hash();
    let block_id = rand::random();

    let mut tx = pool.begin().await.unwrap();

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
    let pool = setup().await;

    let parent = rand::random::<u64>().hash();
    let encoded_locator = rand::random::<u64>().hash();
    let block_id = rand::random();

    let mut tx = pool.begin().await.unwrap();

    let node = LeafNode::present(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    let node = LeafNode::missing(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    let nodes = LeafNode::load_children(&mut tx, &parent).await.unwrap();
    assert_eq!(nodes.len(), 1);

    let node = nodes.get(&encoded_locator).unwrap();
    assert_eq!(node.locator(), &encoded_locator);
    assert_eq!(node.block_id, block_id);
    assert!(!node.is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_empty_leaf_nodes() {
    let pool = setup().await;
    let hash = LeafNodeSet::default().hash();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_incomplete_leaf_nodes() {
    let pool = setup().await;

    let node = LeafNode::missing(rand::random::<u64>().hash(), rand::random());
    let nodes: LeafNodeSet = iter::once(node).collect();
    let hash = nodes.hash();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT)
        .await
        .unwrap();

    assert_eq!(summary, Summary::INCOMPLETE);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_leaf_nodes_with_all_missing_blocks() {
    let pool = setup().await;

    let node = LeafNode::missing(rand::random::<u64>().hash(), rand::random());
    let nodes: LeafNodeSet = iter::once(node).collect();
    let hash = nodes.hash();
    nodes.save(&pool, &hash).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_leaf_nodes_with_some_present_blocks() {
    let pool = setup().await;

    let node0 = LeafNode::present(rand::random::<u64>().hash(), rand::random());
    let node1 = LeafNode::missing(rand::random::<u64>().hash(), rand::random());
    let node2 = LeafNode::missing(rand::random::<u64>().hash(), rand::random());
    let nodes: LeafNodeSet = vec![node0, node1, node2].into_iter().collect();
    let hash = nodes.hash();
    nodes.save(&pool, &hash).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_leaf_nodes_with_all_present_blocks() {
    let pool = setup().await;

    let node0 = LeafNode::present(rand::random::<u64>().hash(), rand::random());
    let node1 = LeafNode::present(rand::random::<u64>().hash(), rand::random());
    let nodes: LeafNodeSet = vec![node0, node1].into_iter().collect();
    let hash = nodes.hash();
    nodes.save(&pool, &hash).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_empty_inner_nodes() {
    let pool = setup().await;
    let hash = InnerNodeMap::default().hash();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT - 1)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_incomplete_inner_nodes() {
    let pool = setup().await;

    let node = InnerNode::new(rand::random::<u64>().hash());
    let nodes: InnerNodeMap = iter::once((0, node)).collect();
    let hash = nodes.hash();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT - 1)
        .await
        .unwrap();

    assert_eq!(summary, Summary::INCOMPLETE);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_inner_nodes_with_all_missing_blocks() {
    let pool = setup().await;

    let inners: InnerNodeMap = (0..2)
        .map(|bucket| {
            let leaf = LeafNode::missing(rand::random::<u64>().hash(), rand::random());
            let leaf_nodes: LeafNodeSet = iter::once(leaf).collect();

            (
                bucket,
                InnerNode {
                    hash: leaf_nodes.hash(),
                    summary: Summary::from_leaves(&leaf_nodes),
                },
            )
        })
        .collect();

    let hash = inners.hash();
    inners.save(&pool, &hash).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT - 1)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_inner_nodes_with_some_present_blocks() {
    let pool = setup().await;

    // all missing
    let inner0 = {
        let leaf_nodes: LeafNodeSet = (0..2)
            .map(|_| LeafNode::missing(rand::random::<u64>().hash(), rand::random()))
            .collect();

        InnerNode {
            hash: leaf_nodes.hash(),
            summary: Summary::from_leaves(&leaf_nodes),
        }
    };

    // some present
    let inner1 = {
        let leaf_nodes: LeafNodeSet = vec![
            LeafNode::missing(rand::random::<u64>().hash(), rand::random()),
            LeafNode::present(rand::random::<u64>().hash(), rand::random()),
        ]
        .into_iter()
        .collect();

        InnerNode {
            hash: leaf_nodes.hash(),
            summary: Summary::from_leaves(&leaf_nodes),
        }
    };

    // all present
    let inner2 = {
        let leaf_nodes: LeafNodeSet = (0..2)
            .map(|_| LeafNode::present(rand::random::<u64>().hash(), rand::random()))
            .collect();

        InnerNode {
            hash: leaf_nodes.hash(),
            summary: Summary::from_leaves(&leaf_nodes),
        }
    };

    let inners: InnerNodeMap = vec![(0, inner0), (1, inner1), (2, inner2)]
        .into_iter()
        .collect();
    let hash = inners.hash();
    inners.save(&pool, &hash).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT - 1)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn compute_status_from_complete_inner_nodes_with_all_present_blocks() {
    let pool = setup().await;

    let inners: InnerNodeMap = (0..2)
        .map(|bucket| {
            let leaf_nodes: LeafNodeSet = (0..2)
                .map(|_| LeafNode::present(rand::random::<u64>().hash(), rand::random()))
                .collect();

            (
                bucket,
                InnerNode {
                    hash: leaf_nodes.hash(),
                    summary: Summary::from_leaves(&leaf_nodes),
                },
            )
        })
        .collect();

    let hash = inners.hash();
    inners.save(&pool, &hash).await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let summary = InnerNode::compute_summary(&mut tx, &hash, INNER_LAYER_COUNT - 1)
        .await
        .unwrap();

    assert!(summary.is_complete);
    assert_eq!(summary.missing_blocks_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_present_on_leaf_node_with_missing_block() {
    let pool = setup().await;
    let mut tx = pool.begin().await.unwrap();

    let parent = rand::random::<u64>().hash();
    let encoded_locator = rand::random::<u64>().hash();
    let block_id = rand::random();

    let node = LeafNode::missing(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    assert!(LeafNode::set_present(&mut tx, &block_id).await.unwrap());

    let nodes = LeafNode::load_children(&mut tx, &parent).await.unwrap();
    assert!(!nodes.get(&encoded_locator).unwrap().is_missing);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_present_on_leaf_node_with_present_block() {
    let pool = setup().await;
    let mut tx = pool.begin().await.unwrap();

    let parent = rand::random::<u64>().hash();
    let encoded_locator = rand::random::<u64>().hash();
    let block_id = rand::random();

    let node = LeafNode::present(encoded_locator, block_id);
    node.save(&mut tx, &parent).await.unwrap();

    assert!(!LeafNode::set_present(&mut tx, &block_id).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn set_present_on_leaf_node_that_does_not_exist() {
    let pool = setup().await;
    let mut tx = pool.begin().await.unwrap();

    let block_id = rand::random();

    assert_matches!(
        LeafNode::set_present(&mut tx, &block_id).await,
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
    let pool = setup().await;

    let replica_id = rng.gen();
    let snapshot = Snapshot::generate(&mut rng, leaf_count);

    let mut root_node = RootNode::create(
        &pool,
        &replica_id,
        VersionVector::new(),
        *snapshot.root_hash(),
        Summary::FULL,
    )
    .await
    .unwrap();

    super::update_summaries(&pool, root_node.hash, 0)
        .await
        .unwrap();
    root_node.reload(&pool).await.unwrap();
    assert_eq!(root_node.summary.is_complete(), leaf_count == 0);

    // TODO: consider randomizing the order the nodes are saved so it's not always
    // breadth-first.

    for layer in snapshot.inner_layers() {
        for (parent_hash, nodes) in layer.inner_maps() {
            nodes.save(&pool, &parent_hash).await.unwrap();
            super::update_summaries(&pool, *parent_hash, layer.number())
                .await
                .unwrap();
            root_node.reload(&pool).await.unwrap();
            assert!(!root_node.summary.is_complete());
        }
    }

    let mut unsaved_leaves = snapshot.leaf_count();

    for (parent_hash, nodes) in snapshot.leaf_sets() {
        nodes.save(&pool, &parent_hash).await.unwrap();
        unsaved_leaves -= nodes.len();

        super::update_summaries(&pool, *parent_hash, INNER_LAYER_COUNT)
            .await
            .unwrap();
        root_node.reload(&pool).await.unwrap();

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
    let pool = setup().await;

    let replica_id = rng.gen();
    let snapshot = Snapshot::generate(&mut rng, leaf_count);

    // Save the snapshot initially with all nodes missing.
    let mut root_node = RootNode::create(
        &pool,
        &replica_id,
        VersionVector::new(),
        *snapshot.root_hash(),
        Summary::INCOMPLETE,
    )
    .await
    .unwrap();

    if snapshot.leaf_count() == 0 {
        super::update_summaries(&pool, root_node.hash, 0)
            .await
            .unwrap();
    }

    for layer in snapshot.inner_layers() {
        for (parent_hash, nodes) in layer.inner_maps() {
            nodes
                .clone()
                .into_incomplete()
                .save(&pool, &parent_hash)
                .await
                .unwrap();
        }
    }

    for (parent_hash, nodes) in snapshot.leaf_sets() {
        nodes
            .clone()
            .into_missing()
            .save(&pool, &parent_hash)
            .await
            .unwrap();

        super::update_summaries(&pool, *parent_hash, INNER_LAYER_COUNT)
            .await
            .unwrap();
    }

    let mut expected_missing_blocks_count = snapshot.leaf_count() as u64;

    // Check that initially all blocks are missing
    root_node.reload(&pool).await.unwrap();
    assert_eq!(
        root_node.summary.missing_blocks_count,
        expected_missing_blocks_count
    );

    // Keep receiving the blocks one by one and verify the missing blocks summaries get updated
    // accordingly.
    for block_id in snapshot.block_ids() {
        let mut tx = pool.begin().await.unwrap();
        super::receive_block(&mut tx, block_id).await.unwrap();
        tx.commit().await.unwrap();

        expected_missing_blocks_count -= 1;

        root_node.reload(&pool).await.unwrap();
        assert_eq!(
            root_node.summary.missing_blocks_count,
            expected_missing_blocks_count
        );

        // TODO: check also inner and leaf nodes
    }
}

async fn setup() -> db::Pool {
    let pool = db::Pool::connect(":memory:").await.unwrap();
    super::super::init(&pool).await.unwrap();
    pool
}

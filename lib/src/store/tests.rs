use super::*;
use crate::{block::BLOCK_SIZE, crypto::cipher::SecretKey, locator::Locator, test_utils};
use proptest::{arbitrary::any, collection::vec};
use rand::{
    rngs::StdRng,
    seq::{IteratorRandom, SliceRandom},
    Rng, SeedableRng,
};
use sqlx::Row;
use std::collections::BTreeMap;
use tempfile::TempDir;
use test_strategy::{proptest, Arbitrary};

#[tokio::test(flavor = "multi_thread")]
async fn link_and_find_block() {
    let (_base_dir, store) = setup().await;
    let branch_id = PublicKey::random();
    let read_key = SecretKey::random();
    let write_keys = Keypair::random();

    let block_id = rand::random();
    let locator = random_head_locator();
    let encoded_locator = locator.encode(&read_key);

    let mut tx = store.begin_write().await.unwrap();

    tx.link_block(
        &branch_id,
        &encoded_locator,
        &block_id,
        SingleBlockPresence::Present,
        &write_keys,
    )
    .await
    .unwrap();
    let (r, _) = tx.find_block(&branch_id, &encoded_locator).await.unwrap();

    assert_eq!(r, block_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn rewrite_locator() {
    for _ in 0..32 {
        let (_base_dir, store) = setup().await;
        let branch_id = PublicKey::random();
        let read_key = SecretKey::random();
        let write_keys = Keypair::random();

        let b1 = rand::random();
        let b2 = rand::random();

        let locator = random_head_locator();
        let encoded_locator = locator.encode(&read_key);

        let mut tx = store.begin_write().await.unwrap();

        tx.link_block(
            &branch_id,
            &encoded_locator,
            &b1,
            SingleBlockPresence::Present,
            &write_keys,
        )
        .await
        .unwrap();

        tx.link_block(
            &branch_id,
            &encoded_locator,
            &b2,
            SingleBlockPresence::Present,
            &write_keys,
        )
        .await
        .unwrap();

        let (r, _) = tx.find_block(&branch_id, &encoded_locator).await.unwrap();
        assert_eq!(r, b2);

        assert_eq!(
            INNER_LAYER_COUNT + 1,
            count_child_nodes(&mut tx).await.unwrap()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_locator() {
    let (_base_dir, store) = setup().await;
    let branch_id = PublicKey::random();
    let read_key = SecretKey::random();
    let write_keys = Keypair::random();

    let b = rand::random();
    let locator = random_head_locator();
    let encoded_locator = locator.encode(&read_key);

    let mut tx = store.begin_write().await.unwrap();

    assert_eq!(0, count_child_nodes(&mut tx).await.unwrap());

    tx.link_block(
        &branch_id,
        &encoded_locator,
        &b,
        SingleBlockPresence::Present,
        &write_keys,
    )
    .await
    .unwrap();
    let (r, _) = tx.find_block(&branch_id, &encoded_locator).await.unwrap();
    assert_eq!(r, b);

    assert_eq!(
        INNER_LAYER_COUNT + 1,
        count_child_nodes(&mut tx).await.unwrap(),
    );

    tx.unlink_block(&branch_id, &encoded_locator, None, &write_keys)
        .await
        .unwrap();

    match tx.find_block(&branch_id, &encoded_locator).await {
        Err(Error::LocatorNotFound) => { /* OK */ }
        Err(_) => panic!("Error should have been LocatorNotFound"),
        Ok(_) => panic!("BranchData shouldn't have contained the block ID"),
    }

    assert_eq!(0, count_child_nodes(&mut tx).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_block() {
    let (_base_dir, store) = setup().await;

    let content = random_block_content();
    let id = BlockId::from_content(&content);
    let nonce = BlockNonce::default();

    let mut tx = store.begin_write().await.unwrap();

    tx.write_block(&id, &content, &nonce).await.unwrap();

    let mut buffer = vec![0; BLOCK_SIZE];
    tx.read_block(&id, &mut buffer).await.unwrap();

    assert_eq!(buffer, content);
}

#[tokio::test(flavor = "multi_thread")]
async fn try_read_missing_block() {
    let (_base_dir, store) = setup().await;

    let mut buffer = vec![0; BLOCK_SIZE];
    let id = BlockId::from_content(&buffer);

    let mut reader = store.acquire_read().await.unwrap();

    match reader.read_block(&id, &mut buffer).await {
        Err(Error::BlockNotFound) => (),
        Err(error) => panic!("unexpected error: {:?}", error),
        Ok(_) => panic!("unexpected success"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn try_write_existing_block() {
    let (_base_dir, store) = setup().await;

    let content0 = random_block_content();
    let id = BlockId::from_content(&content0);
    let nonce = BlockNonce::default();

    let mut tx = store.begin_write().await.unwrap();

    tx.write_block(&id, &content0, &nonce).await.unwrap();
    tx.write_block(&id, &content0, &nonce).await.unwrap();
}

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn fallback() {
    test_utils::init_log();
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, store) = setup().await;
    let branch_0_id = PublicKey::generate(&mut rng);
    let write_keys = Keypair::generate(&mut rng);

    let locator = rng.gen();
    let id0 = rng.gen();
    let id1 = rng.gen();
    let id2 = rng.gen();
    let id3 = rng.gen();

    for (block_id, presence) in [
        (id0, SingleBlockPresence::Present),
        (id1, SingleBlockPresence::Present),
        (id2, SingleBlockPresence::Missing),
        (id3, SingleBlockPresence::Missing),
    ] {
        let mut tx = store.begin_write().await.unwrap();
        // TODO: `link_block` auto-prunes so this doesn't work. We need to simulate receiving
        // remote snapshots here instead.
        tx.link_block(&branch_0_id, &locator, &block_id, presence, &write_keys)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    let root_node = store
        .acquire_read()
        .await
        .unwrap()
        .load_root_node(&branch_0_id)
        .await
        .unwrap();
    store.remove_outdated_snapshots(&root_node).await.unwrap();

    let mut tx = store.begin_read().await.unwrap();

    assert_eq!(
        tx.find_block(&branch_0_id, &locator).await.unwrap(),
        (id3, SingleBlockPresence::Missing)
    );

    // The previous snapshot was pruned because it can't serve as fallback for the latest one
    // but the one before it was not because it can.
    let root_node = tx.load_prev_root_node(&root_node).await.unwrap().unwrap();

    assert_eq!(
        tx.find_block_in(&root_node, &locator).await.unwrap(),
        (id1, SingleBlockPresence::Present)
    );

    // All the further snapshots were pruned as well
    assert!(tx.load_prev_root_node(&root_node).await.unwrap().is_none());
}

#[proptest]
fn empty_nodes_are_not_stored(
    #[strategy(1usize..32)] leaf_count: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(empty_nodes_are_not_stored_case(leaf_count, rng_seed))
}

async fn empty_nodes_are_not_stored_case(leaf_count: usize, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);
    let (_base_dir, store) = setup().await;
    let branch_id = PublicKey::generate(&mut rng);
    let write_keys = Keypair::generate(&mut rng);

    let mut locators = Vec::new();
    let mut tx = store.begin_write().await.unwrap();

    // Add blocks
    for _ in 0..leaf_count {
        let locator = rng.gen();
        let block_id = rng.gen();

        tx.link_block(
            &branch_id,
            &locator,
            &block_id,
            SingleBlockPresence::Present,
            &write_keys,
        )
        .await
        .unwrap();

        locators.push(locator);

        assert!(!has_empty_inner_node(&mut tx).await);
    }

    // Remove blocks
    locators.shuffle(&mut rng);

    for locator in locators {
        tx.unlink_block(&branch_id, &locator, None, &write_keys)
            .await
            .unwrap();

        assert!(!has_empty_inner_node(&mut tx).await);
    }
}

#[proptest]
fn prune(
    #[strategy(vec(any::<PruneTestOp>(), 1..32))] ops: Vec<PruneTestOp>,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(prune_case(ops, rng_seed))
}

#[derive(Arbitrary, Debug)]
enum PruneTestOp {
    Insert,
    Remove,
    Bump,
    Prune,
}

async fn prune_case(ops: Vec<PruneTestOp>, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);
    let (_base_dir, store) = setup().await;
    let branch_id = PublicKey::generate(&mut rng);
    let write_keys = Keypair::generate(&mut rng);

    let mut expected = BTreeMap::new();

    for op in ops {
        // Apply op
        match op {
            PruneTestOp::Insert => {
                let locator = rng.gen();
                let block_id = rng.gen();

                let mut tx = store.begin_write().await.unwrap();
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

                expected.insert(locator, block_id);
            }
            PruneTestOp::Remove => {
                let Some(locator) = expected.keys().choose(&mut rng).copied() else {
                    continue;
                };

                let mut tx = store.begin_write().await.unwrap();
                tx.unlink_block(&branch_id, &locator, None, &write_keys)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();

                expected.remove(&locator);
            }
            PruneTestOp::Bump => {
                let mut tx = store.begin_write().await.unwrap();
                tx.bump(&branch_id, VersionVectorOp::IncrementLocal, &write_keys)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
            PruneTestOp::Prune => {
                let root_node = match store
                    .acquire_read()
                    .await
                    .unwrap()
                    .load_root_node(&branch_id)
                    .await
                {
                    Ok(root_node) => root_node,
                    Err(Error::BranchNotFound) => continue,
                    Err(error) => panic!("unexpected error: {:?}", error),
                };

                store.remove_outdated_snapshots(&root_node).await.unwrap();
            }
        }

        // Verify all expected blocks still present
        let mut tx = store.begin_read().await.unwrap();

        for (locator, expected_block_id) in &expected {
            let (actual_block_id, _) = tx.find_block(&branch_id, locator).await.unwrap();
            assert_eq!(actual_block_id, *expected_block_id);
        }

        // Verify the snapshot is still complete
        let root_hash = tx.load_root_node(&branch_id).await.unwrap().proof.hash;
        check_complete(&mut tx, &root_hash).await;
    }
}

async fn setup() -> (TempDir, Store) {
    let (temp_dir, pool) = db::create_temp().await.unwrap();
    let store = Store::new(pool);
    (temp_dir, store)
}

fn random_block_content() -> Vec<u8> {
    let mut content = vec![0; BLOCK_SIZE];
    rand::thread_rng().fill(&mut content[..]);
    content
}

fn random_head_locator() -> Locator {
    Locator::head(rand::random())
}

/// Counts all child nodes (inner + leaf) in the whole repository.
async fn count_child_nodes(reader: &mut Reader) -> Result<usize, Error> {
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM snapshot_inner_nodes) +
            (SELECT COUNT(*) FROM snapshot_leaf_nodes)",
    )
    .fetch_one(reader.db())
    .await?;

    Ok(row.get::<u32, _>(0) as usize)
}

async fn has_empty_inner_node(reader: &mut Reader) -> bool {
    sqlx::query("SELECT 0 FROM snapshot_inner_nodes WHERE hash = ? LIMIT 1")
        .bind(&*EMPTY_INNER_HASH)
        .fetch_optional(reader.db())
        .await
        .unwrap()
        .is_some()
}

async fn check_complete(reader: &mut Reader, root_hash: &Hash) {
    if *root_hash == *EMPTY_INNER_HASH {
        return;
    }

    let nodes = inner_node::load_children(reader.db(), root_hash)
        .await
        .unwrap();
    assert!(!nodes.is_empty());

    let mut stack: Vec<_> = nodes.into_iter().map(|(_, node)| node).collect();

    while let Some(node) = stack.pop() {
        let inners = inner_node::load_children(reader.db(), &node.hash)
            .await
            .unwrap();
        let leaves = leaf_node::load_children(reader.db(), &node.hash)
            .await
            .unwrap();

        assert!(inners.len() + leaves.len() > 0);

        stack.extend(inners.into_iter().map(|(_, node)| node));
    }
}

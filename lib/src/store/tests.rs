use super::*;
use crate::{
    crypto::{cipher::SecretKey, sign::Keypair},
    protocol::{Block, Bump, Locator, SingleBlockPresence, EMPTY_INNER_HASH, INNER_LAYER_COUNT},
    test_utils,
};
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
    let mut changeset = Changeset::new();
    changeset.link_block(encoded_locator, block_id, SingleBlockPresence::Present);
    changeset
        .apply(&mut tx, &branch_id, &write_keys)
        .await
        .unwrap();

    let r = tx.find_block(&branch_id, &encoded_locator).await.unwrap();

    assert_eq!(r, (block_id, SingleBlockPresence::Present));
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
        let mut changeset = Changeset::new();

        changeset.link_block(encoded_locator, b1, SingleBlockPresence::Present);
        changeset.link_block(encoded_locator, b2, SingleBlockPresence::Present);

        changeset
            .apply(&mut tx, &branch_id, &write_keys)
            .await
            .unwrap();

        let r = tx.find_block(&branch_id, &encoded_locator).await.unwrap();
        assert_eq!(r, (b2, SingleBlockPresence::Present));

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
    let mut changeset = Changeset::new();

    assert_eq!(0, count_child_nodes(&mut tx).await.unwrap());

    changeset.link_block(encoded_locator, b, SingleBlockPresence::Present);
    changeset
        .apply(&mut tx, &branch_id, &write_keys)
        .await
        .unwrap();

    let r = tx.find_block(&branch_id, &encoded_locator).await.unwrap();
    assert_eq!(r, (b, SingleBlockPresence::Present));

    assert_eq!(
        INNER_LAYER_COUNT + 1,
        count_child_nodes(&mut tx).await.unwrap(),
    );

    let mut changeset = Changeset::new();
    changeset.unlink_block(encoded_locator, None);
    // Need to bump otherwise a draft snapshot would be created and the previous snapshot would not
    // be removed.
    changeset.bump(Bump::increment(branch_id));
    changeset
        .apply(&mut tx, &branch_id, &write_keys)
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
async fn remove_block() {
    let (_base_dir, store) = setup().await;

    let read_key = SecretKey::random();
    let write_keys = Keypair::random();

    let branch_id_0 = PublicKey::random();
    let branch_id_1 = PublicKey::random();

    let block: Block = rand::random();
    let block_id = block.id;

    let mut tx = store.begin_write().await.unwrap();
    let mut changeset = Changeset::new();

    changeset.write_block(block);

    let locator0 = Locator::head(rand::random());
    let locator0 = locator0.encode(&read_key);
    changeset.link_block(locator0, block_id, SingleBlockPresence::Present);
    changeset
        .apply(&mut tx, &branch_id_0, &write_keys)
        .await
        .unwrap();

    let locator1 = Locator::head(rand::random());
    let locator1 = locator1.encode(&read_key);
    let mut changeset = Changeset::new();
    changeset.link_block(locator1, block_id, SingleBlockPresence::Present);
    changeset
        .apply(&mut tx, &branch_id_1, &write_keys)
        .await
        .unwrap();

    assert!(tx.block_exists(&block_id).await.unwrap());

    let mut changeset = Changeset::new();
    changeset.unlink_block(locator0, None);
    changeset.bump(Bump::increment(branch_id_0));
    changeset
        .apply(&mut tx, &branch_id_0, &write_keys)
        .await
        .unwrap();

    assert!(tx.block_exists(&block_id).await.unwrap());

    let mut changeset = Changeset::new();
    changeset.unlink_block(locator1, None);
    changeset.bump(Bump::increment(branch_id_0));
    changeset
        .apply(&mut tx, &branch_id_1, &write_keys)
        .await
        .unwrap();

    assert!(!tx.block_exists(&block_id).await.unwrap());
}

#[tokio::test(flavor = "multi_thread")]
async fn overwrite_block() {
    let (_base_dir, store) = setup().await;
    let mut rng = rand::thread_rng();

    let read_key = SecretKey::random();
    let write_keys = Keypair::random();

    let branch_id = PublicKey::random();

    let locator = Locator::head(rng.gen());
    let locator = locator.encode(&read_key);

    let block0: Block = rng.gen();
    let block0_id = block0.id;

    let mut tx = store.begin_write().await.unwrap();

    let mut changeset = Changeset::new();
    changeset.link_block(locator, block0.id, SingleBlockPresence::Present);
    changeset.write_block(block0);
    changeset
        .apply(&mut tx, &branch_id, &write_keys)
        .await
        .unwrap();

    assert!(tx.block_exists(&block0_id).await.unwrap());
    assert_eq!(tx.count_blocks().await.unwrap(), 1);

    let block1: Block = rng.gen();
    let block1_id = block1.id;

    let mut changeset = Changeset::new();
    changeset.write_block(block1);
    changeset.link_block(locator, block1_id, SingleBlockPresence::Present);
    // Need to bump otherwise a draft snapshot would be created and the previous snapshot would not
    // be removed.
    changeset.bump(Bump::increment(branch_id));
    changeset
        .apply(&mut tx, &branch_id, &write_keys)
        .await
        .unwrap();

    assert!(!tx.block_exists(&block0_id).await.unwrap());
    assert!(tx.block_exists(&block1_id).await.unwrap());
    assert_eq!(tx.count_blocks().await.unwrap(), 1);
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
        let mut changeset = Changeset::new();
        // TODO: `link_block` auto-prunes so this doesn't work. We need to simulate receiving
        // remote snapshots here instead.
        changeset.link_block(locator, block_id, presence);

        // TODO: actually create the present blocks

        changeset
            .apply(&mut tx, &branch_0_id, &write_keys)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    let root_node = store
        .acquire_read()
        .await
        .unwrap()
        .load_latest_approved_root_node(&branch_0_id, RootNodeFilter::Any)
        .await
        .unwrap();
    store.remove_outdated_snapshots(&root_node).await.unwrap();

    let mut tx = store.begin_read().await.unwrap();

    assert_eq!(
        tx.find_block(&branch_0_id, &locator).await.unwrap(),
        (id3, SingleBlockPresence::Missing)
    );
    assert!(!tx.block_exists(&id3).await.unwrap());

    // The previous snapshot was pruned because it can't serve as fallback for the latest one
    // but the one before it was not because it can.
    let root_node = tx
        .load_prev_approved_root_node(&root_node)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        tx.find_block_at(&root_node, &locator).await.unwrap(),
        (id1, SingleBlockPresence::Present)
    );
    assert!(tx.block_exists(&id1).await.unwrap());

    // All the further snapshots were pruned as well
    assert!(tx
        .load_prev_approved_root_node(&root_node)
        .await
        .unwrap()
        .is_none());
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

        let mut changeset = Changeset::new();
        changeset.link_block(locator, block_id, SingleBlockPresence::Present);
        changeset
            .apply(&mut tx, &branch_id, &write_keys)
            .await
            .unwrap();

        locators.push(locator);

        assert!(!has_empty_inner_node(&mut tx).await);
    }

    // Remove blocks
    locators.shuffle(&mut rng);

    for locator in locators {
        let mut changeset = Changeset::new();
        changeset.unlink_block(locator, None);
        changeset
            .apply(&mut tx, &branch_id, &write_keys)
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
    let mut approved = false;

    for op in ops {
        // Apply op
        match op {
            PruneTestOp::Insert => {
                let locator = rng.gen();
                let block_id = rng.gen();

                let mut tx = store.begin_write().await.unwrap();
                let mut changeset = Changeset::new();
                changeset.link_block(locator, block_id, SingleBlockPresence::Present);
                changeset
                    .apply(&mut tx, &branch_id, &write_keys)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();

                expected.insert(locator, block_id);
                approved = true;
            }
            PruneTestOp::Remove => {
                let Some(locator) = expected.keys().choose(&mut rng).copied() else {
                    continue;
                };

                let mut tx = store.begin_write().await.unwrap();
                let mut changeset = Changeset::new();
                changeset.unlink_block(locator, None);
                changeset
                    .apply(&mut tx, &branch_id, &write_keys)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();

                expected.remove(&locator);
                approved = true;
            }
            PruneTestOp::Bump => {
                let mut tx = store.begin_write().await.unwrap();
                let mut changeset = Changeset::new();
                changeset.force_bump(true);
                changeset.bump(Bump::increment(branch_id));
                changeset
                    .apply(&mut tx, &branch_id, &write_keys)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
            PruneTestOp::Prune => {
                let root_node = match store
                    .acquire_read()
                    .await
                    .unwrap()
                    .load_latest_approved_root_node(&branch_id, RootNodeFilter::Any)
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
        if approved {
            let root_hash = tx
                .load_latest_approved_root_node(&branch_id, RootNodeFilter::Any)
                .await
                .unwrap()
                .proof
                .hash;
            check_complete(&mut tx, &root_hash).await;
        }
    }
}

async fn setup() -> (TempDir, Store) {
    let (temp_dir, pool) = db::create_temp().await.unwrap();
    let store = Store::new(pool);
    (temp_dir, store)
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

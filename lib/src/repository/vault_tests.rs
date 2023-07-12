use super::{vault::*, LocalId, RepositoryId, RepositoryMonitor};
use crate::{
    access_control::WriteSecrets,
    block::BLOCK_SIZE,
    block::{tracker::OfferState, BlockId, BlockTracker},
    collections::HashSet,
    crypto::{sign::PublicKey, Hash},
    db,
    error::Error,
    event::EventSender,
    index::{
        node_test_utils::{receive_blocks, receive_nodes, Block, Snapshot},
        MultiBlockPresence, Proof, SingleBlockPresence,
    },
    locator::Locator,
    metrics::Metrics,
    progress::Progress,
    state_monitor::StateMonitor,
    store::{self, Store},
    test_utils,
    version_vector::VersionVector,
};
use assert_matches::assert_matches;
use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use std::sync::Arc;
use tempfile::TempDir;
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn receive_valid_blocks() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();
    let receive_filter = vault.store().receive_filter();

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 5);

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        VersionVector::first(branch_id),
        &receive_filter,
        &snapshot,
    )
    .await;
    receive_blocks(&vault, &snapshot).await;

    let mut reader = vault.store().acquire_read().await.unwrap();

    for (id, block) in snapshot.blocks() {
        let mut content = vec![0; BLOCK_SIZE];
        let nonce = reader.read_block(id, &mut content).await.unwrap();

        assert_eq!(&content[..], &block.data.content[..]);
        assert_eq!(nonce, block.nonce);
        assert_eq!(BlockId::from_content(&content), *id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_orphaned_block() {
    let (_base_dir, vault, _secrets) = setup().await;

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
    let block_tracker = vault.block_tracker.client();

    for block in snapshot.blocks().values() {
        vault.block_tracker.require(*block.id());
        block_tracker.offer(*block.id(), OfferState::Approved);
        let promise = block_tracker.acceptor().try_accept().unwrap();

        assert_matches!(
            vault
                .receive_block(&block.data, &block.nonce, Some(promise))
                .await,
            Err(Error::Store(store::Error::BlockNotReferenced))
        );
    }

    let mut reader = vault.store().acquire_read().await.unwrap();
    for id in snapshot.blocks().keys() {
        assert!(!reader.block_exists(id).await.unwrap());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_local() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();

    let locator = Locator::head(rand::random());
    let locator = locator.encode(&secrets.read_key);
    let block_id = rand::random();

    let mut tx = vault.store().begin_write().await.unwrap();
    tx.link_block(
        &branch_id,
        &locator,
        &block_id,
        SingleBlockPresence::Present,
        &secrets.write_keys,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
    let expected = [block_id].into_iter().collect();

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_remote() {
    let (_base_dir, vault, secrets) = setup().await;
    let receive_filter = vault.store().receive_filter();

    let branch_id = PublicKey::random();
    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        VersionVector::first(branch_id),
        &receive_filter,
        &snapshot,
    )
    .await;

    let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
    let expected = snapshot.blocks().keys().copied().collect();

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_excludes_blocks_from_incomplete_snapshots() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();

    // Create snapshot with two leaf nodes but receive only one of them.
    let snapshot = loop {
        let snapshot = Snapshot::generate(&mut rand::thread_rng(), 2);
        if snapshot.leaf_sets().count() > 1 {
            break snapshot;
        }
    };

    let receive_filter = vault.store().receive_filter();
    let version_vector = VersionVector::first(branch_id);
    let proof = Proof::new(
        branch_id,
        version_vector,
        *snapshot.root_hash(),
        &secrets.write_keys,
    );

    vault
        .receive_root_node(proof.into(), MultiBlockPresence::None)
        .await
        .unwrap();

    for layer in snapshot.inner_layers() {
        for (_, nodes) in layer.inner_maps() {
            vault
                .receive_inner_nodes(nodes.clone().into(), &receive_filter, None)
                .await
                .unwrap();
        }
    }

    for (_, nodes) in snapshot.leaf_sets().take(1) {
        vault
            .receive_leaf_nodes(nodes.clone().into(), None)
            .await
            .unwrap();
    }

    let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
    assert!(actual.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_multiple_branches() {
    let (_base_dir, vault, secrets) = setup().await;
    let receive_filter = vault.store().receive_filter();

    let branch_id_0 = PublicKey::random();
    let branch_id_1 = PublicKey::random();

    // One block is common between both branches and one for each branch is unique.
    let all_blocks: [(Hash, Block); 3] = rand::random();
    let blocks_0 = &all_blocks[..2];
    let blocks_1 = &all_blocks[1..];

    let snapshot_0 = Snapshot::new(blocks_0.iter().cloned());
    let snapshot_1 = Snapshot::new(blocks_1.iter().cloned());

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id_0,
        VersionVector::first(branch_id_0),
        &receive_filter,
        &snapshot_0,
    )
    .await;

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id_1,
        VersionVector::first(branch_id_1),
        &receive_filter,
        &snapshot_1,
    )
    .await;

    let actual = vault.store.block_ids(u32::MAX).next().await.unwrap();
    let expected = all_blocks
        .iter()
        .map(|(_, block)| block.id())
        .copied()
        .collect();

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_pagination() {
    let (_base_dir, vault, secrets) = setup().await;
    let receive_filter = vault.store().receive_filter();

    let branch_id = PublicKey::random();
    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 3);

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        VersionVector::first(branch_id),
        &receive_filter,
        &snapshot,
    )
    .await;

    let mut sorted_blocks: Vec<_> = snapshot.blocks().keys().copied().collect();
    sorted_blocks.sort();

    let mut page = vault.store.block_ids(2);

    let actual = page.next().await.unwrap();
    let expected = sorted_blocks[..2].iter().copied().collect();
    assert_eq!(actual, expected);

    let actual = page.next().await.unwrap();
    let expected = sorted_blocks[2..].iter().copied().collect();
    assert_eq!(actual, expected);

    let actual = page.next().await.unwrap();
    assert!(actual.is_empty());
}

#[proptest]
fn sync_progress(
    #[strategy(1usize..16)] block_count: usize,
    #[strategy(1usize..5)] branch_count: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    test_utils::run(sync_progress_case(block_count, branch_count, rng_seed))
}

async fn sync_progress_case(block_count: usize, branch_count: usize, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);

    let (_base_dir, vault, secrets) = setup().await;
    let receive_filter = vault.store().receive_filter();

    let all_blocks: Vec<(Hash, Block)> =
        (&mut rng).sample_iter(Standard).take(block_count).collect();
    let branches: Vec<(PublicKey, Snapshot)> = (0..branch_count)
        .map(|_| {
            let block_count = rng.gen_range(0..block_count);
            let blocks = all_blocks.choose_multiple(&mut rng, block_count).cloned();
            let snapshot = Snapshot::new(blocks);
            let branch_id = PublicKey::generate(&mut rng);

            (branch_id, snapshot)
        })
        .collect();

    let mut expected_total_blocks = HashSet::new();
    let mut expected_received_blocks = HashSet::new();

    assert_eq!(
        vault.store.sync_progress().await.unwrap(),
        Progress {
            value: expected_received_blocks.len() as u64,
            total: expected_total_blocks.len() as u64
        }
    );

    for (branch_id, snapshot) in branches {
        receive_nodes(
            &vault,
            &secrets.write_keys,
            branch_id,
            VersionVector::first(branch_id),
            &receive_filter,
            &snapshot,
        )
        .await;
        expected_total_blocks.extend(snapshot.blocks().keys().copied());

        assert_eq!(
            vault.store.sync_progress().await.unwrap(),
            Progress {
                value: expected_received_blocks.len() as u64,
                total: expected_total_blocks.len() as u64,
            }
        );

        receive_blocks(&vault, &snapshot).await;
        expected_received_blocks.extend(snapshot.blocks().keys().copied());

        assert_eq!(
            vault.store.sync_progress().await.unwrap(),
            Progress {
                value: expected_received_blocks.len() as u64,
                total: expected_total_blocks.len() as u64,
            }
        );
    }

    // HACK: prevent "too many open files" error.
    vault.store().close().await.unwrap();
}

async fn setup() -> (TempDir, Vault, WriteSecrets) {
    setup_with_rng(&mut StdRng::from_entropy()).await
}

async fn setup_with_rng(rng: &mut StdRng) -> (TempDir, Vault, WriteSecrets) {
    let (base_dir, pool) = db::create_temp().await.unwrap();
    let store = Store::new(pool);

    let secrets = WriteSecrets::generate(rng);
    let repository_id = RepositoryId::from(secrets.write_keys.public);

    let event_tx = EventSender::new(1);
    let vault = Vault {
        repository_id,
        store,
        event_tx,
        block_tracker: BlockTracker::new(),
        block_request_mode: BlockRequestMode::Lazy,
        local_id: LocalId::new(),
        monitor: Arc::new(RepositoryMonitor::new(
            StateMonitor::make_root(),
            Metrics::new(),
            "test",
        )),
    };

    (base_dir, vault, secrets)
}

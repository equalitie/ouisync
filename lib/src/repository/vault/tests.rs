use super::{RepositoryId, RepositoryMonitor, Vault};
use crate::{
    access_control::WriteSecrets,
    block_tracker::RequestMode,
    collections::HashSet,
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    event::EventSender,
    progress::Progress,
    protocol::{
        test_utils::Snapshot, Block, Locator, MultiBlockPresence, Proof, RootNodeFilter,
        SingleBlockPresence,
    },
    store::{self, Changeset, ReadTransaction, SnapshotWriter},
    test_utils,
    version_vector::VersionVector,
};
use futures_util::{future, TryStreamExt};
use metrics::NoopRecorder;
use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use state_monitor::StateMonitor;
use tempfile::TempDir;
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn does_not_delete_old_snapshot_until_new_snapshot_is_complete() {
    let (_base_dir, vault, secrets) = setup().await;

    let mut rng = rand::thread_rng();

    let remote_id = PublicKey::generate(&mut rng);

    // Create snapshot v0
    let snapshot0 = Snapshot::generate(&mut rng, 1);
    let vv0 = VersionVector::first(remote_id);

    // Receive it all.
    receive_snapshot(&vault, remote_id, &snapshot0, &secrets.write_keys).await;
    SnapshotWriter::begin(vault.store(), &snapshot0)
        .await
        .save_blocks()
        .await
        .commit()
        .await;

    // Verify we can retrieve all the blocks.
    check_all_blocks_exist(
        &mut vault.store().begin_read().await.unwrap(),
        &remote_id,
        &snapshot0,
    )
    .await;

    // Create snapshot v1
    let snapshot1 = Snapshot::generate(&mut rng, 1);
    let vv1 = vv0.incremented(remote_id);

    // Receive its root node only.
    let mut writer = vault.store().begin_client_write().await.unwrap();
    writer
        .save_root_node(
            Proof::new(remote_id, vv1, *snapshot1.root_hash(), &secrets.write_keys),
            &MultiBlockPresence::Full,
        )
        .await
        .unwrap();
    writer.commit().await.unwrap();

    // All the original blocks are still retrievable
    check_all_blocks_exist(
        &mut vault.store().begin_read().await.unwrap(),
        &remote_id,
        &snapshot0,
    )
    .await;
}

#[tokio::test]
async fn prune_snapshots_insert_present() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, vault, secrets) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = vec![rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&vault, remote_id, &snapshot, &secrets.write_keys).await;
    receive_block(&vault, &blocks[0].1).await;

    // snapshot 2 (insert new block)
    blocks.push(rng.gen());
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&vault, remote_id, &snapshot, &secrets.write_keys).await;
    receive_block(&vault, &blocks[1].1).await;

    assert_eq!(count_snapshots(&vault, &remote_id).await, 2);

    prune_snapshots(&vault, &remote_id).await;

    assert_eq!(count_snapshots(&vault, &remote_id).await, 1);
}

#[tokio::test]
async fn prune_snapshots_insert_missing() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, secrets) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = vec![rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    receive_block(&index, &blocks[0].1).await;

    // snapshot 2 (insert new block)
    blocks.push(rng.gen());
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    // don't receive the new block

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    // snapshot 1 is pruned because even though snapshot 2 has a locator pointing to a missing
    // block, snapshot 1 doesn't have that locator and so can't serve as fallback for snapshot 2.
    assert_eq!(count_snapshots(&index, &remote_id).await, 1);
}

#[tokio::test]
async fn prune_snapshots_update_from_present_to_present() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, secrets) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = [rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    receive_block(&index, &blocks[0].1).await;

    // snapshot 2 (update the first block)
    blocks[0].1 = rng.gen();
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    receive_block(&index, &blocks[0].1).await;

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    assert_eq!(count_snapshots(&index, &remote_id).await, 1);
}

#[tokio::test]
async fn prune_snapshots_update_from_present_to_missing() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, secrets) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = [rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    receive_block(&index, &blocks[0].1).await;

    // snapshot 2 (update the first block)
    blocks[0].1 = rng.gen();
    let snapshot = Snapshot::new(blocks);

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    // don't receive the new block

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    // snapshot 1 is not pruned because snapshot 2 has a locator pointing to a missing block while
    // in snapshot 1 the same locator points to a present block and so snapshot 1 can serve as
    // fallback for snapshot 2.
    assert_eq!(count_snapshots(&index, &remote_id).await, 2);
}

#[tokio::test]
async fn prune_snapshots_update_from_missing_to_missing() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, secrets) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = [rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    // don't receive the block

    // snapshot 2 (update the first block)
    blocks[0].1 = rng.gen();
    let snapshot = Snapshot::new(blocks);

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    // don't receive the new block

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    // snapshot 1 is pruned because even though snapshot 2 has a locator pointing to a missing
    // block, the same locator is also pointing to a missing block in snapshot 1 and so snapshot 1
    // can't serve as fallback for snapshot 2.
    assert_eq!(count_snapshots(&index, &remote_id).await, 1);
}

#[tokio::test]
async fn prune_snapshots_keep_missing_and_insert_missing() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, secrets) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = vec![rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    // don't receive the block

    // snapshot 2 (insert new block)
    blocks.push(rng.gen());
    let snapshot = Snapshot::new(blocks);

    receive_snapshot(&index, remote_id, &snapshot, &secrets.write_keys).await;
    // don't receive the new block

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    // snapshot 1 is pruned because even though snapshot 2 has locators pointing to a missing
    // blocks, one of the locators points to the same missing block also in snapshot 1 and the
    // other one doesn't even exists in snapshot 1. Thus, snapshot 1 can't serve as fallback for
    // snapshot 2.
    assert_eq!(count_snapshots(&index, &remote_id).await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_local() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();

    let locator = Locator::head(rand::random());
    let locator = locator.encode(&secrets.read_key);
    let block_id = rand::random();

    let mut tx = vault.store().begin_write().await.unwrap();
    let mut changeset = Changeset::new();
    changeset.link_block(locator, block_id, SingleBlockPresence::Present);
    changeset
        .apply(&mut tx, &branch_id, &secrets.write_keys)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let actual = vault.store().block_ids(u32::MAX).next().await.unwrap();
    let expected = [block_id].into_iter().collect();

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_remote() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();
    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

    SnapshotWriter::begin(vault.store(), &snapshot)
        .await
        .save_nodes(
            &secrets.write_keys,
            branch_id,
            VersionVector::first(branch_id),
        )
        .await
        .save_blocks()
        .await
        .commit()
        .await;

    let actual = vault.store().block_ids(u32::MAX).next().await.unwrap();
    let expected = snapshot.blocks().keys().copied().collect();

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_excludes_missing_blocks() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();
    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

    SnapshotWriter::begin(vault.store(), &snapshot)
        .await
        .save_nodes(
            &secrets.write_keys,
            branch_id,
            VersionVector::first(branch_id),
        )
        .await
        .commit()
        .await;

    let actual = vault.store().block_ids(u32::MAX).next().await.unwrap();
    assert!(actual.is_empty());
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

    let version_vector = VersionVector::first(branch_id);
    let proof = Proof::new(
        branch_id,
        version_vector,
        *snapshot.root_hash(),
        &secrets.write_keys,
    );

    let mut writer = vault.store().begin_client_write().await.unwrap();

    writer
        .save_root_node(proof, &MultiBlockPresence::None)
        .await
        .unwrap();

    for layer in snapshot.inner_layers() {
        for (_, nodes) in layer.inner_maps() {
            writer.save_inner_nodes(nodes.clone().into()).await.unwrap();
        }
    }

    for (_, nodes) in snapshot.leaf_sets().take(1) {
        writer.save_leaf_nodes(nodes.clone().into()).await.unwrap();
    }

    writer.commit().await.unwrap();

    let actual = vault.store().block_ids(u32::MAX).next().await.unwrap();
    assert!(actual.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_multiple_branches() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id_0 = PublicKey::random();
    let branch_id_1 = PublicKey::random();

    // One block is common between both branches and one for each branch is unique.
    let all_blocks: [(Hash, Block); 3] = rand::random();
    let blocks_0 = &all_blocks[..2];
    let blocks_1 = &all_blocks[1..];

    let snapshot_0 = Snapshot::new(blocks_0.iter().cloned());
    let snapshot_1 = Snapshot::new(blocks_1.iter().cloned());

    SnapshotWriter::begin(vault.store(), &snapshot_0)
        .await
        .save_nodes(
            &secrets.write_keys,
            branch_id_0,
            VersionVector::first(branch_id_0),
        )
        .await
        .commit()
        .await;

    SnapshotWriter::begin(vault.store(), &snapshot_1)
        .await
        .save_nodes(
            &secrets.write_keys,
            branch_id_1,
            VersionVector::first(branch_id_1),
        )
        .await
        .commit()
        .await;

    SnapshotWriter::begin(vault.store(), &snapshot_0)
        .await
        .save_blocks()
        .await
        .commit()
        .await;

    SnapshotWriter::begin(vault.store(), &snapshot_1)
        .await
        .save_blocks()
        .await
        .commit()
        .await;

    let actual = vault.store().block_ids(u32::MAX).next().await.unwrap();
    let expected = all_blocks
        .iter()
        .map(|(_, block)| &block.id)
        .copied()
        .collect();

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_pagination() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();
    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 3);

    SnapshotWriter::begin(vault.store(), &snapshot)
        .await
        .save_nodes(
            &secrets.write_keys,
            branch_id,
            VersionVector::first(branch_id),
        )
        .await
        .save_blocks()
        .await
        .commit()
        .await;

    let mut sorted_blocks: Vec<_> = snapshot.blocks().keys().copied().collect();
    sorted_blocks.sort();

    let mut page = vault.store().block_ids(2);

    let actual = page.next().await.unwrap();
    let expected = sorted_blocks[..2].iter().copied().collect();
    assert_eq!(actual, expected);

    let actual = page.next().await.unwrap();
    let expected = sorted_blocks[2..].iter().copied().collect();
    assert_eq!(actual, expected);

    let actual = page.next().await.unwrap();
    assert!(actual.is_empty());
}

#[proptest(async = "tokio")]
async fn sync_progress(
    #[strategy(1usize..16)] block_count: usize,
    #[strategy(1usize..5)] branch_count: usize,
    #[strategy(test_utils::rng_seed_strategy())] rng_seed: u64,
) {
    sync_progress_case(block_count, branch_count, rng_seed).await
}

async fn sync_progress_case(block_count: usize, branch_count: usize, rng_seed: u64) {
    let mut rng = StdRng::seed_from_u64(rng_seed);

    let (_base_dir, vault, secrets) = setup().await;

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
        vault.store().sync_progress().await.unwrap(),
        Progress {
            value: expected_received_blocks.len() as u64,
            total: expected_total_blocks.len() as u64
        }
    );

    for (branch_id, snapshot) in branches {
        SnapshotWriter::begin(vault.store(), &snapshot)
            .await
            .save_nodes(
                &secrets.write_keys,
                branch_id,
                VersionVector::first(branch_id),
            )
            .await
            .commit()
            .await;
        expected_total_blocks.extend(snapshot.blocks().keys().copied());

        assert_eq!(
            vault.store().sync_progress().await.unwrap(),
            Progress {
                value: expected_received_blocks.len() as u64,
                total: expected_total_blocks.len() as u64,
            }
        );

        SnapshotWriter::begin(vault.store(), &snapshot)
            .await
            .save_blocks()
            .await
            .commit()
            .await;
        expected_received_blocks.extend(snapshot.blocks().keys().copied());

        assert_eq!(
            vault.store().sync_progress().await.unwrap(),
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

    let secrets = WriteSecrets::generate(rng);
    let repository_id = RepositoryId::from(secrets.write_keys.public_key());

    let vault = Vault::new(
        repository_id,
        EventSender::new(1),
        pool,
        RepositoryMonitor::new(StateMonitor::make_root(), &NoopRecorder),
    );

    vault.block_tracker.set_request_mode(RequestMode::Lazy);

    (base_dir, vault, secrets)
}

async fn receive_snapshot(
    vault: &Vault,
    writer_id: PublicKey,
    snapshot: &Snapshot,
    write_keys: &Keypair,
) {
    let vv = match vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_latest_approved_root_node(&writer_id, RootNodeFilter::Any)
        .await
    {
        Ok(node) => node.proof.into_version_vector(),
        Err(store::Error::BranchNotFound) => VersionVector::new(),
        Err(error) => panic!("unexpected error: {:?}", error),
    };
    let vv = vv.incremented(writer_id);

    SnapshotWriter::begin(vault.store(), snapshot)
        .await
        .save_nodes(write_keys, writer_id, vv)
        .await
        .commit()
        .await;
}

async fn receive_block(vault: &Vault, block: &Block) {
    let mut writer = vault.store().begin_client_write().await.unwrap();
    writer.save_block(block, None).await.unwrap();
    writer.commit().await.unwrap();
}

async fn check_all_blocks_exist(
    tx: &mut ReadTransaction,
    branch_id: &PublicKey,
    snapshot: &Snapshot,
) {
    for node in snapshot.leaf_sets().flat_map(|(_, nodes)| nodes) {
        let (block_id, _) = tx.find_block(branch_id, &node.locator).await.unwrap();
        assert!(tx.block_exists(&block_id).await.unwrap());
    }
}

async fn count_snapshots(vault: &Vault, writer_id: &PublicKey) -> usize {
    vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_root_nodes_by_writer(writer_id)
        .try_fold(0, |sum, _node| future::ready(Ok(sum + 1)))
        .await
        .unwrap()
}

async fn prune_snapshots(vault: &Vault, writer_id: &PublicKey) {
    let root_node = vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_latest_approved_root_node(writer_id, RootNodeFilter::Any)
        .await
        .unwrap();
    vault
        .store()
        .remove_outdated_snapshots(&root_node)
        .await
        .unwrap();
}

use super::{BlockRequestMode, RepositoryId, RepositoryMonitor, Vault};
use crate::{
    access_control::WriteSecrets,
    block_tracker::OfferState,
    collections::HashSet,
    crypto::{
        sign::{Keypair, PublicKey},
        Hash,
    },
    db,
    error::Error,
    event::EventSender,
    progress::Progress,
    protocol::{
        test_utils::{receive_blocks, receive_nodes, Snapshot},
        Block, BlockContent, BlockId, Locator, MultiBlockPresence, NodeState, Proof,
        RootNodeFilter, SingleBlockPresence, EMPTY_INNER_HASH,
    },
    store::{self, Changeset, ReadTransaction},
    test_utils,
    version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::{future, StreamExt, TryStreamExt};
use metrics::NoopRecorder;
use rand::{distributions::Standard, rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};
use state_monitor::StateMonitor;
use tempfile::TempDir;
use test_strategy::proptest;

#[tokio::test(flavor = "multi_thread")]
async fn receive_valid_root_node() {
    let (_base_dir, vault, secrets) = setup().await;
    let remote_id = PublicKey::random();

    // Initially the remote branch doesn't exist
    assert!(vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_root_nodes_by_writer(&remote_id)
        .try_next()
        .await
        .unwrap()
        .is_none());

    // Receive root node from the remote replica.
    vault
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::first(remote_id),
                *EMPTY_INNER_HASH,
                &secrets.write_keys,
            )
            .into(),
            MultiBlockPresence::None,
        )
        .await
        .unwrap();

    // The remote branch now exist.
    assert!(vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_root_nodes_by_writer(&remote_id)
        .try_next()
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_invalid_proof() {
    let (_base_dir, vault, _) = setup().await;
    let remote_id = PublicKey::random();

    // Receive invalid root node from the remote replica.
    let invalid_write_keys = Keypair::random();
    let status = vault
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::first(remote_id),
                *EMPTY_INNER_HASH,
                &invalid_write_keys,
            )
            .into(),
            MultiBlockPresence::None,
        )
        .await
        .unwrap();
    assert!(status.new_approved.is_empty());
    assert!(!status.request_children);

    // The invalid root was not written to the db.
    assert!(vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_root_nodes_by_writer(&remote_id)
        .try_next()
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_empty_version_vector() {
    let (_base_dir, vault, secrets) = setup().await;
    let remote_id = PublicKey::random();

    vault
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::new(),
                *EMPTY_INNER_HASH,
                &secrets.write_keys,
            )
            .into(),
            MultiBlockPresence::None,
        )
        .await
        .unwrap();

    assert!(vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_root_nodes_by_writer(&remote_id)
        .try_next()
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_duplicate_root_node() {
    let (_base_dir, vault, secrets) = setup().await;
    let remote_id = PublicKey::random();

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
    let proof = Proof::new(
        remote_id,
        VersionVector::first(remote_id),
        *snapshot.root_hash(),
        &secrets.write_keys,
    );

    // Receive root node for the first time.
    vault
        .receive_root_node(proof.clone().into(), MultiBlockPresence::None)
        .await
        .unwrap();

    // Receiving it again is a no-op.
    vault
        .receive_root_node(proof.into(), MultiBlockPresence::None)
        .await
        .unwrap();

    assert_eq!(
        vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id)
            .filter(|node| future::ready(node.is_ok()))
            .count()
            .await,
        1
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_existing_hash() {
    let (_base_dir, vault, secrets) = setup().await;
    let mut rng = rand::thread_rng();

    let local_id = PublicKey::generate(&mut rng);
    let remote_id = PublicKey::generate(&mut rng);

    // Create one block locally
    let block: Block = rng.gen();
    let locator = rng.gen();

    let mut tx = vault.store().begin_write().await.unwrap();
    let mut changeset = Changeset::new();

    changeset.link_block(locator, block.id, SingleBlockPresence::Present);
    changeset.write_block(block);
    changeset
        .apply(&mut tx, &local_id, &secrets.write_keys)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Receive root node with the same hash as the current local one but different writer id.
    let root = vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_latest_approved_root_node(&local_id, RootNodeFilter::Any)
        .await
        .unwrap();

    assert!(root.summary.state.is_approved());
    let root_hash = root.proof.hash;
    let root_vv = root.proof.version_vector.clone();

    let proof = Proof::new(remote_id, root_vv, root_hash, &secrets.write_keys);

    // TODO: assert this returns false as we shouldn't need to download further nodes
    vault
        .receive_root_node(proof.into(), MultiBlockPresence::None)
        .await
        .unwrap();

    assert!(vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_latest_approved_root_node(&local_id, RootNodeFilter::Any)
        .await
        .unwrap()
        .summary
        .state
        .is_approved());
}

mod receive_and_create_root_node {
    use super::*;
    use crate::protocol::Bump;
    use tokio::task;

    #[tokio::test(flavor = "multi_thread")]
    async fn local_then_remove() {
        case(TaskOrder::LocalThenRemote).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remote_then_local() {
        case(TaskOrder::RemoteThenLocal).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent() {
        case(TaskOrder::Concurrent).await
    }

    enum TaskOrder {
        LocalThenRemote,
        RemoteThenLocal,
        Concurrent,
    }

    async fn case(order: TaskOrder) {
        let mut rng = StdRng::seed_from_u64(0);
        let (_base_dir, vault, secrets) = setup_with_rng(&mut rng).await;

        let local_id = PublicKey::generate(&mut rng);

        let locator_0 = rng.gen();
        let block_id_0_0 = rng.gen();
        let block_id_0_1 = rng.gen();

        let locator_1 = rng.gen();
        let block_1: Block = rng.gen();

        let locator_2 = rng.gen();
        let block_id_2 = rng.gen();

        // Insert one present and two missing, so the root block presence is `Some`
        let mut tx = vault.store().begin_write().await.unwrap();
        let mut changeset = Changeset::new();

        for (locator, block_id, presence) in [
            (locator_0, block_id_0_0, SingleBlockPresence::Present),
            (locator_1, block_1.id, SingleBlockPresence::Missing),
            (locator_2, block_id_2, SingleBlockPresence::Missing),
        ] {
            changeset.link_block(locator, block_id, presence);
        }

        changeset
            .apply(&mut tx, &local_id, &secrets.write_keys)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let root_node_0 = vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&local_id)
            .try_next()
            .await
            .unwrap()
            .unwrap();

        // Mark one of the missing block as present so the block presences are different (but still
        // `Some`).
        let mut tx = vault.store().begin_write().await.unwrap();
        tx.receive_block(&block_1).await.unwrap();
        tx.commit().await.unwrap();

        // Receive the same node we already have. The hashes and version vectors are equal but the
        // block presences are different (and both are `Some`) so the received node is considered
        // up-to-date.
        let remote_task = async {
            vault
                .receive_root_node(
                    root_node_0.proof.clone().into(),
                    root_node_0.summary.block_presence,
                )
                .await
                .unwrap();
        };

        // Create a new snapshot locally
        let local_task = async {
            // This transaction will block `remote_task` until it is committed.
            let mut tx = vault.store().begin_write().await.unwrap();

            // yield a bit to give `remote_task` chance to run until it needs to begin its own
            // transaction.
            for _ in 0..100 {
                task::yield_now().await;
            }

            let mut changeset = Changeset::new();
            changeset.link_block(locator_0, block_id_0_1, SingleBlockPresence::Present);
            changeset.bump(Bump::increment(local_id));
            changeset
                .apply(&mut tx, &local_id, &secrets.write_keys)
                .await
                .unwrap();

            tx.commit().await.unwrap();
        };

        match order {
            TaskOrder::LocalThenRemote => {
                local_task.await;
                remote_task.await;
            }
            TaskOrder::RemoteThenLocal => {
                remote_task.await;
                local_task.await;
            }
            TaskOrder::Concurrent => {
                future::join(remote_task, local_task).await;
            }
        }

        let root_node_1 = vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&local_id)
            .try_next()
            .await
            .unwrap()
            .unwrap();

        // In all three cases the locally created snapshot must be newer than the received one:
        // - In the local-then-remote case, the remote is outdated by the time it's received and so
        //   it's not even inserted.
        // - In the remote-then-local case, the remote one is inserted first but then the local one
        //   overwrites it
        // - In the concurrent case, the remote is still up-to-date when its started to get received
        //   but the local is holding a db transaction so the remote can't proceed until the local one
        //   commits it, and so by the time the transaction is committed, the remote is no longer
        //   up-to-date.
        assert!(root_node_1.proof.version_vector > root_node_0.proof.version_vector);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_bumped_root_node() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
    let vv0 = VersionVector::first(branch_id);

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        vv0.clone(),
        &snapshot,
    )
    .await;

    let node = vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_latest_approved_root_node(&branch_id, RootNodeFilter::Any)
        .await
        .unwrap();
    assert_eq!(node.proof.version_vector, vv0);
    assert_eq!(node.summary.state, NodeState::Approved);

    // Receive root node with the same hash as before but greater vv.
    let vv1 = vv0.incremented(branch_id);
    vault
        .receive_root_node(
            Proof::new(
                branch_id,
                vv1.clone(),
                *snapshot.root_hash(),
                &secrets.write_keys,
            )
            .into(),
            MultiBlockPresence::None,
        )
        .await
        .unwrap();

    let node = vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_latest_approved_root_node(&branch_id, RootNodeFilter::Any)
        .await
        .unwrap();
    assert_eq!(node.proof.version_vector, vv1);
    assert_eq!(node.summary.state, NodeState::Approved);
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_valid_child_nodes() {
    let (_base_dir, vault, secrets) = setup().await;
    let remote_id = PublicKey::random();

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

    vault
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::first(remote_id),
                *snapshot.root_hash(),
                &secrets.write_keys,
            )
            .into(),
            MultiBlockPresence::None,
        )
        .await
        .unwrap();

    for layer in snapshot.inner_layers() {
        for (hash, inner_nodes) in layer.inner_maps() {
            vault
                .receive_inner_nodes(inner_nodes.clone().into(), None)
                .await
                .unwrap();

            assert!(!vault
                .store()
                .acquire_read()
                .await
                .unwrap()
                .load_inner_nodes(hash)
                .await
                .unwrap()
                .is_empty());
        }
    }

    for (hash, leaf_nodes) in snapshot.leaf_sets() {
        vault
            .receive_leaf_nodes(leaf_nodes.clone().into(), None)
            .await
            .unwrap();

        assert!(!vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_leaf_nodes(hash)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_child_nodes_with_missing_root_parent() {
    let (_base_dir, vault, _secrets) = setup().await;

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

    for layer in snapshot.inner_layers() {
        let (hash, inner_nodes) = layer.inner_maps().next().unwrap();
        let status = vault
            .receive_inner_nodes(inner_nodes.clone().into(), None)
            .await
            .unwrap();
        assert!(status.new_approved.is_empty());
        assert!(status.request_children.is_empty());

        // The orphaned inner nodes were not written to the db.
        let inner_nodes = vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_inner_nodes(hash)
            .await
            .unwrap();
        assert!(inner_nodes.is_empty());
    }

    let (hash, leaf_nodes) = snapshot.leaf_sets().next().unwrap();
    let status = vault
        .receive_leaf_nodes(leaf_nodes.clone().into(), None)
        .await
        .unwrap();
    assert!(!status.old_approved);
    assert!(status.new_approved.is_empty());
    assert!(status.request_blocks.is_empty());

    // The orphaned leaf nodes were not written to the db.
    let leaf_nodes = vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .load_leaf_nodes(hash)
        .await
        .unwrap();
    assert!(leaf_nodes.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_valid_blocks() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();
    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 5);

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        VersionVector::first(branch_id),
        &snapshot,
    )
    .await;
    receive_blocks(&vault, &snapshot).await;

    let mut reader = vault.store().acquire_read().await.unwrap();

    for (id, block) in snapshot.blocks() {
        let mut content = BlockContent::new();
        let nonce = reader.read_block(id, &mut content).await.unwrap();

        assert_eq!(&content[..], &block.content[..]);
        assert_eq!(nonce, block.nonce);
        assert_eq!(BlockId::new(&content, &nonce), *id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_orphaned_block() {
    let (_base_dir, vault, _secrets) = setup().await;

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
    let block_tracker = vault.block_tracker.client();

    for block in snapshot.blocks().values() {
        vault.block_tracker.require(block.id);
        block_tracker.register(block.id, OfferState::Approved);
        let promise = block_tracker.offers().try_next().unwrap().accept().unwrap();

        assert_matches!(
            vault.receive_block(block, Some(promise)).await,
            Err(Error::Store(store::Error::BlockNotReferenced))
        );
    }

    let mut reader = vault.store().acquire_read().await.unwrap();
    for id in snapshot.blocks().keys() {
        assert!(!reader.block_exists(id).await.unwrap());
    }
}

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
    receive_blocks(&vault, &snapshot0).await;

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
    vault
        .receive_root_node(
            Proof::new(remote_id, vv1, *snapshot1.root_hash(), &secrets.write_keys).into(),
            MultiBlockPresence::Full,
        )
        .await
        .unwrap();

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

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        VersionVector::first(branch_id),
        &snapshot,
    )
    .await;
    receive_blocks(&vault, &snapshot).await;

    let actual = vault.store().block_ids(u32::MAX).next().await.unwrap();
    let expected = snapshot.blocks().keys().copied().collect();

    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn block_ids_excludes_missing_blocks() {
    let (_base_dir, vault, secrets) = setup().await;

    let branch_id = PublicKey::random();
    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        VersionVector::first(branch_id),
        &snapshot,
    )
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

    vault
        .receive_root_node(proof.into(), MultiBlockPresence::None)
        .await
        .unwrap();

    for layer in snapshot.inner_layers() {
        for (_, nodes) in layer.inner_maps() {
            vault
                .receive_inner_nodes(nodes.clone().into(), None)
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

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id_0,
        VersionVector::first(branch_id_0),
        &snapshot_0,
    )
    .await;

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id_1,
        VersionVector::first(branch_id_1),
        &snapshot_1,
    )
    .await;

    receive_blocks(&vault, &snapshot_0).await;
    receive_blocks(&vault, &snapshot_1).await;

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

    receive_nodes(
        &vault,
        &secrets.write_keys,
        branch_id,
        VersionVector::first(branch_id),
        &snapshot,
    )
    .await;
    receive_blocks(&vault, &snapshot).await;

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
        receive_nodes(
            &vault,
            &secrets.write_keys,
            branch_id,
            VersionVector::first(branch_id),
            &snapshot,
        )
        .await;
        expected_total_blocks.extend(snapshot.blocks().keys().copied());

        assert_eq!(
            vault.store().sync_progress().await.unwrap(),
            Progress {
                value: expected_received_blocks.len() as u64,
                total: expected_total_blocks.len() as u64,
            }
        );

        receive_blocks(&vault, &snapshot).await;
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
        BlockRequestMode::Lazy,
        RepositoryMonitor::new(StateMonitor::make_root(), &NoopRecorder),
    );

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

    receive_nodes(vault, write_keys, writer_id, vv, snapshot).await
}

async fn receive_block(vault: &Vault, block: &Block) {
    let mut tx = vault.store().begin_write().await.unwrap();
    tx.receive_block(block).await.unwrap();
    tx.commit().await.unwrap();
}

async fn check_all_blocks_exist(
    tx: &mut ReadTransaction,
    branch_id: &PublicKey,
    snapshot: &Snapshot,
) {
    for node in snapshot.leaf_sets().flat_map(|(_, nodes)| nodes) {
        let block_id = tx.find_block(branch_id, &node.locator).await.unwrap();
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

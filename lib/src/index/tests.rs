use super::{
    branch_data::BranchData,
    node::{self, InnerNode, LeafNode, RootNode, SingleBlockPresence, Summary, EMPTY_INNER_HASH},
    node_test_utils::Snapshot,
    proof::Proof,
    Index, ReceiveError, ReceiveFilter,
};
use crate::{
    block::{self, BlockId, BlockTracker, BLOCK_SIZE},
    crypto::sign::{Keypair, PublicKey},
    db,
    repository::{LocalId, RepositoryId},
    store::{BlockRequestMode, Store},
    version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::{future, StreamExt, TryStreamExt};
use rand::{rngs::StdRng, Rng, SeedableRng};
use tempfile::TempDir;
use tokio::{sync::broadcast, task};
use tracing::Span;

#[tokio::test(flavor = "multi_thread")]
async fn receive_valid_root_node() {
    let (_base_dir, index, write_keys) = setup().await;
    let remote_id = PublicKey::random();
    let mut conn = index.pool.acquire().await.unwrap();

    // Initially the remote branch doesn't exist
    assert!(RootNode::load_latest_by_writer(&mut conn, remote_id)
        .await
        .unwrap()
        .is_none());

    // Receive root node from the remote replica.
    index
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::first(remote_id),
                *EMPTY_INNER_HASH,
                &write_keys,
            )
            .into(),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();

    // The remote branch now exist.
    assert!(RootNode::load_latest_by_writer(&mut conn, remote_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_invalid_proof() {
    let (_base_dir, index, _) = setup().await;
    let remote_id = PublicKey::random();

    // Receive invalid root node from the remote replica.
    let invalid_write_keys = Keypair::random();
    let result = index
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::first(remote_id),
                *EMPTY_INNER_HASH,
                &invalid_write_keys,
            )
            .into(),
            Summary::INCOMPLETE,
        )
        .await;
    assert_matches!(result, Err(ReceiveError::InvalidProof));

    // The invalid root was not written to the db.
    let mut conn = index.pool.acquire().await.unwrap();
    assert!(RootNode::load_latest_by_writer(&mut conn, remote_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_empty_version_vector() {
    let (_base_dir, index, write_keys) = setup().await;
    let remote_id = PublicKey::random();

    index
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::new(),
                *EMPTY_INNER_HASH,
                &write_keys,
            )
            .into(),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();

    let mut conn = index.pool.acquire().await.unwrap();
    assert!(RootNode::load_latest_by_writer(&mut conn, remote_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_duplicate_root_node() {
    let (_base_dir, index, write_keys) = setup().await;
    let remote_id = PublicKey::random();

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
    let proof = Proof::new(
        remote_id,
        VersionVector::first(remote_id),
        *snapshot.root_hash(),
        &write_keys,
    );

    // Receive root node for the first time.
    index
        .receive_root_node(proof.clone().into(), Summary::INCOMPLETE)
        .await
        .unwrap();

    // Receiving it again is a no-op.
    index
        .receive_root_node(proof.into(), Summary::INCOMPLETE)
        .await
        .unwrap();

    assert_eq!(
        RootNode::load_all_by_writer(&mut index.pool.acquire().await.unwrap(), remote_id)
            .filter(|node| future::ready(node.is_ok()))
            .count()
            .await,
        1
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_existing_hash() {
    let (_base_dir, index, write_keys) = setup().await;
    let mut rng = rand::thread_rng();

    let local_id = PublicKey::generate(&mut rng);
    let remote_id = PublicKey::generate(&mut rng);

    let local_branch = index.get_branch(local_id);

    // Create one block locally
    let mut content = vec![0; BLOCK_SIZE];
    rng.fill(&mut content[..]);

    let block_id = BlockId::from_content(&content);
    let block_nonce = rng.gen();
    let locator = rng.gen();

    let mut tx = index.pool.begin().await.unwrap();

    block::write(&mut tx, &block_id, &content, &block_nonce)
        .await
        .unwrap();
    local_branch
        .insert(
            &mut tx,
            &locator,
            &block_id,
            SingleBlockPresence::Present,
            &write_keys,
        )
        .await
        .unwrap();

    tx.commit().await.unwrap();

    let mut conn = index.pool.acquire().await.unwrap();

    // Receive root node with the same hash as the current local one but different writer id.
    let root = local_branch
        .load_snapshot(&mut conn)
        .await
        .unwrap()
        .root_node;

    assert!(root.summary.is_complete());
    let root_hash = root.proof.hash;
    let root_vv = root.proof.version_vector.clone();

    let proof = Proof::new(remote_id, root_vv, root_hash, &write_keys);

    // TODO: assert this returns false as we shouldn't need to download further nodes
    index
        .receive_root_node(proof.into(), Summary::FULL)
        .await
        .unwrap();

    assert!(local_branch
        .load_snapshot(&mut conn)
        .await
        .unwrap()
        .root_node
        .summary
        .is_complete());
}

mod receive_and_create_root_node {
    use super::*;

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
        let (_base_dir, index, write_keys) = setup_with_rng(&mut rng).await;

        let local_id = PublicKey::generate(&mut rng);
        let local_branch = index.get_branch(local_id);

        let locator_0 = rng.gen();
        let block_id_0_0 = rng.gen();
        let block_id_0_1 = rng.gen();

        let locator_1 = rng.gen();
        let block_id_1 = rng.gen();

        let locator_2 = rng.gen();
        let block_id_2 = rng.gen();

        // Insert one present and two missing, so the root block presence is `Some`
        let mut tx = index.pool.begin().await.unwrap();

        for (locator, block_id, presence) in [
            (locator_0, block_id_0_0, SingleBlockPresence::Present),
            (locator_1, block_id_1, SingleBlockPresence::Missing),
            (locator_2, block_id_2, SingleBlockPresence::Missing),
        ] {
            local_branch
                .insert(&mut tx, &locator, &block_id, presence, &write_keys)
                .await
                .unwrap();
        }

        tx.commit().await.unwrap();

        let mut conn = index.pool.acquire().await.unwrap();
        let root_node_0 = RootNode::load_latest_by_writer(&mut conn, *local_branch.id())
            .await
            .unwrap()
            .unwrap();
        drop(conn);

        // Mark one of the missing block as present so the block presences are different (but still
        // `Some`).
        let mut tx = index.pool.begin().await.unwrap();
        node::receive_block(&mut tx, &block_id_1).await.unwrap();
        tx.commit().await.unwrap();

        // Receive the same node we already have. The hashes and version vectors are equal but the
        // block presences are different (and both are `Some`) so the received node is considered
        // up-to-date.
        let remote_task = async {
            index
                .receive_root_node(root_node_0.proof.clone().into(), root_node_0.summary)
                .await
                .unwrap();
        };

        // Create a new snapshot locally
        let local_task = async {
            // This transaction will block `remote_task` until it is committed.
            let mut tx = index.pool.begin().await.unwrap();

            // yield a bit to give `remote_task` chance to run until it needs to begin its own
            // transaction.
            for _ in 0..100 {
                task::yield_now().await;
            }

            local_branch
                .insert(
                    &mut tx,
                    &locator_0,
                    &block_id_0_1,
                    SingleBlockPresence::Present,
                    &write_keys,
                )
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

        let mut conn = index.pool.acquire().await.unwrap();
        let root_node_1 = RootNode::load_latest_by_writer(&mut conn, *local_branch.id())
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
async fn receive_valid_child_nodes() {
    let (_base_dir, index, write_keys) = setup().await;
    let remote_id = PublicKey::random();

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);

    index
        .receive_root_node(
            Proof::new(
                remote_id,
                VersionVector::first(remote_id),
                *snapshot.root_hash(),
                &write_keys,
            )
            .into(),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();

    let mut receive_filter = ReceiveFilter::new(index.pool.clone());

    for layer in snapshot.inner_layers() {
        for (hash, inner_nodes) in layer.inner_maps() {
            index
                .receive_inner_nodes(inner_nodes.clone().into(), &mut receive_filter)
                .await
                .unwrap();

            assert!(
                !InnerNode::load_children(&mut index.pool.acquire().await.unwrap(), hash)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    for (hash, leaf_nodes) in snapshot.leaf_sets() {
        index
            .receive_leaf_nodes(leaf_nodes.clone().into())
            .await
            .unwrap();

        assert!(
            !LeafNode::load_children(&mut index.pool.acquire().await.unwrap(), hash)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_child_nodes_with_missing_root_parent() {
    let (_base_dir, index, _write_keys) = setup().await;

    let snapshot = Snapshot::generate(&mut rand::thread_rng(), 1);
    let mut receive_filter = ReceiveFilter::new(index.pool.clone());

    for layer in snapshot.inner_layers() {
        let (hash, inner_nodes) = layer.inner_maps().next().unwrap();
        let result = index
            .receive_inner_nodes(inner_nodes.clone().into(), &mut receive_filter)
            .await;
        assert_matches!(result, Err(ReceiveError::ParentNodeNotFound));

        // The orphaned inner nodes were not written to the db.
        let inner_nodes = InnerNode::load_children(&mut index.pool.acquire().await.unwrap(), hash)
            .await
            .unwrap();
        assert!(inner_nodes.is_empty());
    }

    let (hash, leaf_nodes) = snapshot.leaf_sets().next().unwrap();
    let result = index.receive_leaf_nodes(leaf_nodes.clone().into()).await;
    assert_matches!(result, Err(ReceiveError::ParentNodeNotFound));

    // The orphaned leaf nodes were not written to the db.
    let leaf_nodes = LeafNode::load_children(&mut index.pool.acquire().await.unwrap(), hash)
        .await
        .unwrap();
    assert!(leaf_nodes.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_delete_old_snapshot_until_new_snapshot_is_complete() {
    let (_base_dir, index, write_keys) = setup().await;
    let store = Store {
        index,
        block_tracker: BlockTracker::new(),
        block_request_mode: BlockRequestMode::Lazy,
        local_id: LocalId::new(),
        span: Span::none(),
    };

    let mut rng = rand::thread_rng();

    let remote_id = PublicKey::generate(&mut rng);

    // Create snapshot v0
    let snapshot0 = Snapshot::generate(&mut rng, 1);
    let vv0 = VersionVector::first(remote_id);

    // Receive it all.
    receive_snapshot(&store.index, remote_id, &snapshot0, &write_keys).await;

    for block in snapshot0.blocks().values() {
        store
            .write_received_block(&block.data, &block.nonce)
            .await
            .unwrap();
    }

    let remote_branch = store.index.get_branch(remote_id);

    // Verify we can retrieve all the blocks.
    check_all_blocks_exist(
        &mut store.db().acquire().await.unwrap(),
        &remote_branch,
        &snapshot0,
    )
    .await;

    // Create snapshot v1
    let snapshot1 = Snapshot::generate(&mut rng, 1);
    let vv1 = vv0.incremented(remote_id);

    // Receive its root node only.
    store
        .index
        .receive_root_node(
            Proof::new(remote_id, vv1, *snapshot1.root_hash(), &write_keys).into(),
            Summary::FULL,
        )
        .await
        .unwrap();

    // All the original blocks are still retrievable
    check_all_blocks_exist(
        &mut store.db().acquire().await.unwrap(),
        &remote_branch,
        &snapshot0,
    )
    .await;
}

#[tokio::test]
async fn prune_snapshots_insert_present() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, write_keys) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = vec![rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    receive_block(&index, blocks[0].1.id()).await;

    // snapshot 2 (insert new block)
    blocks.push(rng.gen());
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    receive_block(&index, blocks[1].1.id()).await;

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    assert_eq!(count_snapshots(&index, &remote_id).await, 1);
}

#[tokio::test]
async fn prune_snapshots_insert_missing() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, write_keys) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = vec![rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    receive_block(&index, blocks[0].1.id()).await;

    // snapshot 2 (insert new block)
    blocks.push(rng.gen());
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
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
    let (_base_dir, index, write_keys) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = [rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    receive_block(&index, blocks[0].1.id()).await;

    // snapshot 2 (update the first block)
    blocks[0].1 = rng.gen();
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    receive_block(&index, blocks[0].1.id()).await;

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    assert_eq!(count_snapshots(&index, &remote_id).await, 1);
}

#[tokio::test]
async fn prune_snapshots_update_from_present_to_missing() {
    let mut rng = StdRng::seed_from_u64(0);
    let (_base_dir, index, write_keys) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = [rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    receive_block(&index, blocks[0].1.id()).await;

    // snapshot 2 (update the first block)
    blocks[0].1 = rng.gen();
    let snapshot = Snapshot::new(blocks);

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
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
    let (_base_dir, index, write_keys) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = [rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    // don't receive the block

    // snapshot 2 (update the first block)
    blocks[0].1 = rng.gen();
    let snapshot = Snapshot::new(blocks);

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
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
    let (_base_dir, index, write_keys) = setup_with_rng(&mut rng).await;

    let remote_id = PublicKey::generate(&mut rng);

    // snapshot 1
    let mut blocks = vec![rng.gen()];
    let snapshot = Snapshot::new(blocks.clone());

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    // don't receive the block

    // snapshot 2 (insert new block)
    blocks.push(rng.gen());
    let snapshot = Snapshot::new(blocks);

    receive_snapshot(&index, remote_id, &snapshot, &write_keys).await;
    // don't receive the new block

    assert_eq!(count_snapshots(&index, &remote_id).await, 2);

    prune_snapshots(&index, &remote_id).await;

    // snapshot 1 is pruned because even though snapshot 2 has locators pointing to a missing
    // blocks, one of the locators points to the same missing block also in snapshot 1 and the
    // other one doesn't even exists in snapshot 1. Thus, snapshot 1 can't serve as fallback for
    // snapshot 2.
    assert_eq!(count_snapshots(&index, &remote_id).await, 1);
}

async fn setup() -> (TempDir, Index, Keypair) {
    setup_with_rng(&mut StdRng::from_entropy()).await
}

async fn setup_with_rng(rng: &mut StdRng) -> (TempDir, Index, Keypair) {
    let (base_dir, pool) = db::create_temp().await.unwrap();

    let write_keys = Keypair::generate(rng);
    let repository_id = RepositoryId::from(write_keys.public);
    let (event_tx, _) = broadcast::channel(1);
    let index = Index::new(pool, repository_id, event_tx);

    (base_dir, index, write_keys)
}

async fn check_all_blocks_exist(
    conn: &mut db::Connection,
    branch: &BranchData,
    snapshot: &Snapshot,
) {
    for node in snapshot.leaf_sets().flat_map(|(_, nodes)| nodes) {
        let (block_id, _) = branch.get(conn, &node.locator).await.unwrap();
        assert!(block::exists(conn, &block_id).await.unwrap());
    }
}

async fn receive_snapshot(
    index: &Index,
    writer_id: PublicKey,
    snapshot: &Snapshot,
    write_keys: &Keypair,
) {
    let vv = {
        let mut conn = index.pool.acquire().await.unwrap();
        index
            .get_branch(writer_id)
            .load_version_vector(&mut conn)
            .await
            .unwrap()
            .incremented(writer_id)
    };

    index
        .receive_root_node(
            Proof::new(writer_id, vv, *snapshot.root_hash(), write_keys).into(),
            Summary::FULL,
        )
        .await
        .unwrap();

    let mut receive_filter = ReceiveFilter::new(index.pool.clone());

    for layer in snapshot.inner_layers() {
        for (_, nodes) in layer.inner_maps() {
            index
                .receive_inner_nodes(nodes.clone().into(), &mut receive_filter)
                .await
                .unwrap();
        }
    }

    for (_, nodes) in snapshot.leaf_sets() {
        index
            .receive_leaf_nodes(nodes.clone().into())
            .await
            .unwrap();
    }
}

async fn receive_block(index: &Index, block_id: &BlockId) {
    let mut tx = index.pool.begin().await.unwrap();
    node::receive_block(&mut tx, block_id).await.unwrap();
    tx.commit().await.unwrap();
}

async fn count_snapshots(index: &Index, writer_id: &PublicKey) -> usize {
    let mut conn = index.pool.acquire().await.unwrap();
    RootNode::load_all_by_writer(&mut conn, *writer_id)
        .try_fold(0, |sum, _node| future::ready(Ok(sum + 1)))
        .await
        .unwrap()
}

async fn prune_snapshots(index: &Index, writer_id: &PublicKey) {
    let mut conn = index.pool.acquire().await.unwrap();
    let snapshot = index
        .get_branch(*writer_id)
        .load_snapshot(&mut conn)
        .await
        .unwrap();
    drop(conn);

    snapshot.prune(&index.pool).await.unwrap();
}

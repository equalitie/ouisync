use super::{node::RootNode, node_test_utils::Snapshot, *};
use crate::{
    block::{self, BlockTracker, BLOCK_SIZE},
    crypto::sign::{Keypair, PublicKey},
    store::Store,
    version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::{future, StreamExt};
use rand::{rngs::StdRng, Rng, SeedableRng};
use sqlx::Connection;

#[tokio::test(flavor = "multi_thread")]
async fn receive_valid_root_node() {
    let (index, write_keys) = setup().await;

    let local_id = PublicKey::random();
    let remote_id = PublicKey::random();

    index
        .create_branch(Proof::first(local_id, &write_keys))
        .await
        .unwrap();

    // Initially only the local branch exists
    let mut conn = index.pool.acquire().await.unwrap();
    assert!(RootNode::load_latest_by_writer(&mut conn, local_id)
        .await
        .unwrap()
        .is_some());
    assert!(RootNode::load_latest_by_writer(&mut conn, remote_id)
        .await
        .unwrap()
        .is_none());
    drop(conn);

    // Receive root node from the remote replica.
    index
        .receive_root_node(
            Proof::first(remote_id, &write_keys).into(),
            Summary::INCOMPLETE,
        )
        .await
        .unwrap();

    // Both the local and the remote branch now exist.
    let mut conn = index.pool.acquire().await.unwrap();
    assert!(RootNode::load_latest_by_writer(&mut conn, local_id)
        .await
        .unwrap()
        .is_some());
    assert!(RootNode::load_latest_by_writer(&mut conn, remote_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_invalid_proof() {
    let (index, write_keys) = setup().await;

    let local_id = PublicKey::random();
    let remote_id = PublicKey::random();

    index
        .create_branch(Proof::first(local_id, &write_keys))
        .await
        .unwrap();

    // Receive invalid root node from the remote replica.
    let invalid_write_keys = Keypair::random();
    let result = index
        .receive_root_node(
            Proof::first(remote_id, &invalid_write_keys).into(),
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
async fn receive_duplicate_root_node() {
    let (index, write_keys) = setup().await;

    let local_id = PublicKey::random();
    let remote_id = PublicKey::random();

    index
        .create_branch(Proof::first(local_id, &write_keys))
        .await
        .unwrap();

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
        RootNode::load_all_by_writer(&mut index.pool.acquire().await.unwrap(), remote_id, 2)
            .filter(|node| future::ready(node.is_ok()))
            .count()
            .await,
        1
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_root_node_with_existing_hash() {
    let (index, write_keys) = setup().await;
    let mut rng = rand::thread_rng();

    let local_id = PublicKey::generate(&mut rng);
    let remote_id = PublicKey::generate(&mut rng);

    let local_branch = index
        .create_branch(Proof::first(local_id, &write_keys))
        .await
        .unwrap();

    // Create one block locally
    let mut content = vec![0; BLOCK_SIZE];
    rng.fill(&mut content[..]);

    let block_id = BlockId::from_content(&content);
    let block_nonce = rng.gen();
    let locator = rng.gen();

    let mut conn = index.pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();

    block::write(&mut tx, &block_id, &content, &block_nonce)
        .await
        .unwrap();
    local_branch
        .insert(&mut tx, &block_id, &locator, &write_keys)
        .await
        .unwrap();

    tx.commit().await.unwrap();

    // Receive root node with the same hash as the current local one but different writer id.
    let root = local_branch.load_root(&mut conn).await.unwrap();
    drop(conn);

    assert!(root.summary.is_complete());
    let root_hash = root.proof.hash;
    let root_vv = root.proof.version_vector.clone();

    let proof = Proof::new(remote_id, root_vv, root_hash, &write_keys);

    // TODO: assert this returns false as we shouldn't need to download further nodes
    index
        .receive_root_node(proof.into(), Summary::FULL)
        .await
        .unwrap();

    let mut conn = index.pool.acquire().await.unwrap();
    assert!(local_branch
        .load_root(&mut conn)
        .await
        .unwrap()
        .summary
        .is_complete());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_valid_child_nodes() {
    let (index, write_keys) = setup().await;

    let local_id = PublicKey::random();
    let remote_id = PublicKey::random();

    index
        .create_branch(Proof::first(local_id, &write_keys))
        .await
        .unwrap();

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
    let (index, write_keys) = setup().await;

    let local_id = PublicKey::random();

    index
        .create_branch(Proof::first(local_id, &write_keys))
        .await
        .unwrap();

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
async fn does_not_delete_old_branch_until_new_branch_is_complete() {
    let (index, write_keys) = setup().await;
    let store = Store {
        index,
        block_tracker: BlockTracker::lazy(),
    };

    let mut rng = rand::thread_rng();

    let local_id = PublicKey::generate(&mut rng);
    store
        .index
        .create_branch(Proof::first(local_id, &write_keys))
        .await
        .unwrap();

    let remote_id = PublicKey::generate(&mut rng);

    // Create snapshot v0
    let snapshot0 = Snapshot::generate(&mut rng, 1);
    let vv0 = VersionVector::first(remote_id);

    // Receive it all.
    store
        .index
        .receive_root_node(
            Proof::new(remote_id, vv0.clone(), *snapshot0.root_hash(), &write_keys).into(),
            Summary::FULL,
        )
        .await
        .unwrap();

    let mut receive_filter = ReceiveFilter::new(store.db().clone());

    for layer in snapshot0.inner_layers() {
        for (_, nodes) in layer.inner_maps() {
            store
                .index
                .receive_inner_nodes(nodes.clone().into(), &mut receive_filter)
                .await
                .unwrap();
        }
    }

    for (_, nodes) in snapshot0.leaf_sets() {
        store
            .index
            .receive_leaf_nodes(nodes.clone().into())
            .await
            .unwrap();
    }

    for block in snapshot0.blocks().values() {
        store
            .write_received_block(&block.data, &block.nonce)
            .await
            .unwrap();
    }

    let remote_branch = store.index.get_branch(&remote_id).unwrap();

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

async fn setup() -> (Index, Keypair) {
    setup_with_rng(&mut StdRng::from_entropy()).await
}

async fn setup_with_rng(rng: &mut StdRng) -> (Index, Keypair) {
    let pool = db::create(&db::Store::Temporary).await.unwrap();

    let write_keys = Keypair::generate(rng);
    let repository_id = RepositoryId::from(write_keys.public);
    let (event_tx, _) = broadcast::channel(1);
    let index = Index::load(pool, repository_id, event_tx).await.unwrap();

    (index, write_keys)
}

async fn check_all_blocks_exist(
    conn: &mut db::Connection,
    branch: &BranchData,
    snapshot: &Snapshot,
) {
    for node in snapshot.leaf_sets().flat_map(|(_, nodes)| nodes) {
        let block_id = branch.get(conn, node.locator()).await.unwrap();
        assert!(block::exists(conn, &block_id).await.unwrap());
    }
}

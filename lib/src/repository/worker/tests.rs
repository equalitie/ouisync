use super::{
    super::{Credentials, RepositoryMonitor, Shared},
    prune, unlock,
};
use crate::{
    access_control::AccessSecrets, crypto::sign, db, event::Payload,
    protocol::test_utils::Snapshot, store::SnapshotWriter, version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::TryStreamExt;
use metrics::NoopRecorder;
use rand::rngs::StdRng;
use state_monitor::StateMonitor;
use tempfile::TempDir;

#[tokio::test]
async fn prune_sanity_check() {
    let mut rng = rand::make_rng::<StdRng>();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let writer_a = sign::PublicKey::generate(&mut rng);
    let writer_b = sign::PublicKey::generate(&mut rng);
    let writer_c = sign::PublicKey::generate(&mut rng);
    let writer_d = sign::PublicKey::generate(&mut rng);

    tracing::debug!(?writer_a, ?writer_b, ?writer_c, ?writer_d);

    // D is outdated to A and B and has only incomplete snapshots
    let mut vv_d = VersionVector::first(writer_d);
    for _ in 0..2 {
        let snapshot = Snapshot::generate(&mut rng, 2);

        SnapshotWriter::begin(shared.vault.store(), &snapshot)
            .await
            .save_root_nodes(&secrets.write_keys, writer_d, vv_d.clone())
            .await
            .save_inner_nodes()
            .await
            // Do not save leaf nodes, to keep the snapshot incomplete.
            .commit()
            .await;

        vv_d.increment(writer_d);
    }

    // C is outdated to A and B and has complete snapshots
    let mut vv_c = VersionVector::first(writer_c);
    for _ in 0..2 {
        let snapshot = Snapshot::generate(&mut rng, 2);

        SnapshotWriter::begin(shared.vault.store(), &snapshot)
            .await
            .save_nodes(&secrets.write_keys, writer_c, vv_c.clone())
            .await
            .commit()
            .await;

        vv_c.increment(writer_c);
    }

    // A and B are concurrent
    for writer_id in [writer_a, writer_b] {
        let mut vv = vv_c.clone().merged(&vv_d).incremented(writer_id);

        for _ in 0..2 {
            let snapshot = Snapshot::generate(&mut rng, 2);

            SnapshotWriter::begin(shared.vault.store(), &snapshot)
                .await
                .save_nodes(&secrets.write_keys, writer_id, vv.clone())
                .await
                .commit()
                .await;

            vv.increment(writer_id);
        }
    }

    // Verify all snapshots are present
    let mut reader = shared.vault.store().acquire_read().await.unwrap();
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_a)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_b)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_c)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_d)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
    drop(reader);

    // Prune
    let (unlock_tx, _unlock_rx) = unlock::channel();
    prune::run(&shared, &unlock_tx).await.unwrap();

    // Verify only the latest snapshots of A and B are present
    let mut reader = shared.vault.store().acquire_read().await.unwrap();
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_a)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_])
    );
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_b)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_])
    );
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_c)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([])
    );
    assert_matches!(
        reader
            .load_root_nodes_by_writer(&writer_d)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([])
    );
}

#[tokio::test]
async fn do_not_prune_complete_snapshots_if_no_newer_snapshot_is_complete() {
    let mut rng: StdRng = rand::make_rng();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let writer = sign::PublicKey::generate(&mut rng);
    let mut vv = VersionVector::first(writer);

    // Create complete snapshot
    let snapshot = Snapshot::generate(&mut rng, 1);
    SnapshotWriter::begin(shared.vault.store(), &snapshot)
        .await
        .save_root_nodes(&secrets.write_keys, writer, vv.clone())
        .await
        .save_inner_nodes()
        .await
        .save_leaf_nodes()
        .await
        .commit()
        .await;

    vv.increment(writer);

    // Create newer incomplete snapshot
    let snapshot = Snapshot::generate(&mut rng, 1);
    SnapshotWriter::begin(shared.vault.store(), &snapshot)
        .await
        .save_root_nodes(&secrets.write_keys, writer, vv.clone())
        .await
        .save_inner_nodes()
        .await
        .commit()
        .await;

    vv.increment(writer);

    // Verify all snapshots are present
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&writer)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );

    // Prune
    let (unlock_tx, _unlock_rx) = unlock::channel();
    prune::run(&shared, &unlock_tx).await.unwrap();

    // Verify all snapshots are still present
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&writer)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
}

#[tokio::test]
async fn do_not_prune_incomplete_snapshots_if_no_newer_snapshot_is_complete() {
    let mut rng: StdRng = rand::make_rng();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let writer = sign::PublicKey::generate(&mut rng);
    let mut vv = VersionVector::first(writer);

    for _ in 0..2 {
        let snapshot = Snapshot::generate(&mut rng, 2);

        SnapshotWriter::begin(shared.vault.store(), &snapshot)
            .await
            .save_root_nodes(&secrets.write_keys, writer, vv.clone())
            .await
            .save_inner_nodes()
            .await
            // Do not save leaf nodes, to keep the snapshot incomplete.
            .commit()
            .await;

        vv.increment(writer);
    }

    // Verify all snapshots are present
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&writer)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );

    // Prune
    let (unlock_tx, _unlock_rx) = unlock::channel();
    prune::run(&shared, &unlock_tx).await.unwrap();

    // Verify all snapshots are still present
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&writer)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
}

// A branch can have a complete snapshot followed by a newer incomplete one. When such branch
// becomes outdated, all its snapshots must be pruned, including the incomplete one (it's also
// outdated and there might be no other prune run to remove it).
#[tokio::test]
async fn prune_outdated_branch_with_newer_incomplete_snapshot() {
    let mut rng: StdRng = rand::make_rng();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let local_id = shared.credentials.read().unwrap().writer_id;
    let remote_id = sign::PublicKey::generate(&mut rng);

    // Remote: complete snapshot
    let remote_vv_0 = VersionVector::first(remote_id);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(&secrets.write_keys, remote_id, remote_vv_0.clone())
        .await
        .commit()
        .await;

    // Remote: newer, incomplete snapshot
    let remote_vv_1 = remote_vv_0.incremented(remote_id);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_root_nodes(&secrets.write_keys, remote_id, remote_vv_1.clone())
        .await
        .save_inner_nodes()
        .await
        // Do not save leaf nodes, to keep the snapshot incomplete.
        .commit()
        .await;

    // Local: complete snapshot, newer than both remote snapshots
    let local_vv = remote_vv_1.incremented(local_id);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(&secrets.write_keys, local_id, local_vv)
        .await
        .commit()
        .await;

    // Verify all remote snapshots are present
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );

    // Prune
    let (unlock_tx, _unlock_rx) = unlock::channel();
    prune::run(&shared, &unlock_tx).await.unwrap();

    // Verify all remote snapshots have been removed
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([])
    );
}

// A branch has an approved snapshot and a newer incomplete one. Another branch has an incomplete
// snapshot which is newer than both of them. The approved snapshot must not be pruned because the
// only snapshots that supersede it are not approved (and might never be).
#[tokio::test]
async fn do_not_prune_approved_snapshot_superseded_only_by_non_approved_ones() {
    let mut rng: StdRng = rand::make_rng();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let local_id = shared.credentials.read().unwrap().writer_id;
    let remote_id_a = sign::PublicKey::generate(&mut rng);
    let remote_id_b = sign::PublicKey::generate(&mut rng);

    // Remote A: complete snapshot
    let vv_a0 = VersionVector::first(remote_id_a);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(&secrets.write_keys, remote_id_a, vv_a0.clone())
        .await
        .commit()
        .await;

    // Remote A: newer, incomplete snapshot
    let vv_a1 = vv_a0.incremented(remote_id_a);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_root_nodes(&secrets.write_keys, remote_id_a, vv_a1.clone())
        .await
        .save_inner_nodes()
        .await
        // Do not save leaf nodes, to keep the snapshot incomplete.
        .commit()
        .await;

    // Remote B: incomplete snapshot, newer than both snapshots of A
    let vv_b = vv_a1.incremented(remote_id_b);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_root_nodes(&secrets.write_keys, remote_id_b, vv_b)
        .await
        .save_inner_nodes()
        .await
        // Do not save leaf nodes, to keep the snapshot incomplete.
        .commit()
        .await;

    // Local: complete snapshot, concurrent with all the remote ones
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(
            &secrets.write_keys,
            local_id,
            VersionVector::first(local_id),
        )
        .await
        .commit()
        .await;

    // Prune
    let (unlock_tx, _unlock_rx) = unlock::channel();
    prune::run(&shared, &unlock_tx).await.unwrap();

    // Verify all snapshots of A are still present
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&remote_id_a)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
}

// The local branch can contain a non-approved snapshot (e.g., received from another replica that
// uses the same writer id). Such snapshot is treated as non-local for the purpose of pruning, but
// the local branch itself must still never be pruned.
#[tokio::test]
async fn do_not_prune_local_branch_with_non_approved_snapshot() {
    let mut rng: StdRng = rand::make_rng();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let local_id = shared.credentials.read().unwrap().writer_id;
    let remote_id = sign::PublicKey::generate(&mut rng);

    // Local: complete snapshot
    let local_vv_0 = VersionVector::first(local_id);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(&secrets.write_keys, local_id, local_vv_0.clone())
        .await
        .commit()
        .await;

    // Local: newer, incomplete snapshot
    let local_vv_1 = local_vv_0.incremented(local_id);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_root_nodes(&secrets.write_keys, local_id, local_vv_1.clone())
        .await
        .save_inner_nodes()
        .await
        // Do not save leaf nodes, to keep the snapshot incomplete.
        .commit()
        .await;

    // Remote: complete snapshot, newer than both local ones
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(
            &secrets.write_keys,
            remote_id,
            local_vv_1.incremented(remote_id),
        )
        .await
        .commit()
        .await;

    // Prune
    let (unlock_tx, _unlock_rx) = unlock::channel();
    prune::run(&shared, &unlock_tx).await.unwrap();

    // Verify all local snapshots are still present
    assert_matches!(
        shared
            .vault
            .store()
            .acquire_read()
            .await
            .unwrap()
            .load_root_nodes_by_writer(&local_id)
            .try_collect::<Vec<_>>()
            .await
            .as_deref(),
        Ok([_, _])
    );
}

// Removing an outdated branch emits `BranchRemoved` (which, among other things, restarts the scan
// job, which might otherwise fail due to the branch being removed while being scanned).
#[tokio::test]
async fn prune_outdated_branch_emits_branch_removed_event() {
    let mut rng: StdRng = rand::make_rng();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let local_id = shared.credentials.read().unwrap().writer_id;
    let remote_id = sign::PublicKey::generate(&mut rng);

    // Remote: complete snapshot
    let remote_vv = VersionVector::first(remote_id);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(&secrets.write_keys, remote_id, remote_vv.clone())
        .await
        .commit()
        .await;

    // Local: complete snapshot, newer than the remote one
    let local_vv = remote_vv.incremented(local_id);
    SnapshotWriter::begin(shared.vault.store(), &Snapshot::generate(&mut rng, 2))
        .await
        .save_nodes(&secrets.write_keys, local_id, local_vv)
        .await
        .commit()
        .await;

    let mut event_rx = shared.vault.event_tx.subscribe();

    // Prune
    let (unlock_tx, _unlock_rx) = unlock::channel();
    prune::run(&shared, &unlock_tx).await.unwrap();

    let mut removed = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        if let Payload::BranchRemoved(branch_id) = event.payload {
            removed.push(branch_id);
        }
    }

    assert_eq!(removed, [remote_id]);
}

async fn setup(rng: &mut StdRng) -> (TempDir, Shared) {
    crate::test_utils::init_log();

    let (base_dir, pool) = db::create_temp().await.unwrap();
    let secrets = AccessSecrets::generate_write(rng);
    let writer_id = sign::Keypair::generate(rng).public_key();
    let credentials = Credentials { secrets, writer_id };

    let shared = Shared::new(
        pool,
        credentials,
        RepositoryMonitor::new(StateMonitor::make_root(), &NoopRecorder),
    );

    (base_dir, shared)
}

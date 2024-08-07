use super::{
    super::{Credentials, RepositoryMonitor, Shared},
    prune, unlock,
    utils::Counter,
};
use crate::{
    access_control::AccessSecrets, crypto::sign, db, protocol::test_utils::Snapshot,
    store::SnapshotWriter, version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::TryStreamExt;
use metrics::NoopRecorder;
use rand::{rngs::StdRng, SeedableRng};
use state_monitor::StateMonitor;
use tempfile::TempDir;

#[tokio::test]
async fn prune() {
    let mut rng = StdRng::from_entropy();
    let (_base_dir, shared) = setup(&mut rng).await;
    let secrets = shared
        .credentials
        .read()
        .unwrap()
        .secrets
        .clone()
        .into_write_secrets()
        .unwrap();

    let writer_a = sign::Keypair::generate(&mut rng).public_key();
    let writer_b = sign::Keypair::generate(&mut rng).public_key();
    let writer_c = sign::Keypair::generate(&mut rng).public_key();
    let writer_d = sign::Keypair::generate(&mut rng).public_key();

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
    prune::run(&shared, &unlock_tx, &Counter::new())
        .await
        .unwrap();

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

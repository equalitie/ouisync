use super::{
    super::{Credentials, RepositoryMonitor, Shared},
    prune, unlock,
    utils::Counter,
};
use crate::{
    access_control::AccessSecrets,
    crypto::sign,
    db,
    protocol::test_utils::{self, Snapshot},
    version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::TryStreamExt;
use metrics::NoopRecorder;
use rand::{rngs::StdRng, SeedableRng};
use state_monitor::StateMonitor;
use tempfile::TempDir;

#[ignore] // FIXME: prune doesn't currently remove branches with no complete snapshots
#[tokio::test]
async fn prune_outdated_branches_with_only_incomplete_snapshots() {
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

    // { A: 1, B: 1 }
    let vv_a = VersionVector::first(writer_a).incremented(writer_b);
    // { A: 0, B: 1}
    let vv_b = VersionVector::first(writer_b);

    let snapshot_a = Snapshot::generate(&mut rng, 2);
    let snapshot_b = Snapshot::generate(&mut rng, 2);

    // Receive complete snapshot of A
    test_utils::receive_nodes(
        &shared.vault,
        &secrets.write_keys,
        writer_a,
        vv_a,
        &snapshot_a,
    )
    .await;

    // Receive incomplete snapshot of B
    test_utils::receive_root_nodes(
        &shared.vault,
        &secrets.write_keys,
        writer_b,
        vv_b,
        &snapshot_b,
    )
    .await;
    test_utils::receive_inner_nodes(&shared.vault, &snapshot_b).await;
    // Do not receive leaf nodes, to keep the snapshot incomplete.

    // Verify both branches are present
    let mut reader = shared.vault.store().acquire_read().await.unwrap();
    assert_matches!(
        reader.load_root_nodes_by_writer(&writer_a).try_next().await,
        Ok(Some(_))
    );
    assert_matches!(
        reader.load_root_nodes_by_writer(&writer_b).try_next().await,
        Ok(Some(_))
    );
    drop(reader);

    let (unlock_tx, _unlock_rx) = unlock::channel();

    // Prune
    prune::run(&shared, &unlock_tx, &Counter::new())
        .await
        .unwrap();

    // Verify only the A branch is present
    let mut reader = shared.vault.store().acquire_read().await.unwrap();
    assert_matches!(
        reader.load_root_nodes_by_writer(&writer_a).try_next().await,
        Ok(Some(_))
    );
    assert_matches!(
        reader.load_root_nodes_by_writer(&writer_b).try_next().await,
        Ok(None)
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

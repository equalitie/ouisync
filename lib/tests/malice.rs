//! Tests for resilience to malicious behaviour / attacks.

#[macro_use]
mod common;

use common::{actor, Env, Proto, DEFAULT_REPO};
use ouisync::{AccessMode, Repository};
use tokio::sync::mpsc;

#[ignore] // FIXME: https://github.com/equalitie/ouisync/issues/116
#[test]
fn block_nonce_tamper() {
    let mut env = Env::new();

    let content = b"hello world";

    // Side channels to coordinate the replicas.
    let (alice_to_mallory_tx, mut alice_to_mallory_rx) = mpsc::channel(1);
    let (mallory_to_bob_tx, mut mallory_to_bob_rx) = mpsc::channel(1);
    let (bob_to_mallory_tx, mut bob_to_mallory_rx) = mpsc::channel(1);
    let (bob_to_alice_tx, mut bob_to_alice_rx) = mpsc::channel(1);

    // Alice is a writer who creates the file initially.
    env.actor("alice", async move {
        let (_network, repo, _reg) = actor::setup().await;

        let mut file = repo.create_file("test.txt").await.unwrap();
        file.write_all(content).await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let local_branch = repo.local_branch().unwrap();
        let local_id = *local_branch.id();
        let local_vv = local_branch.version_vector().await.unwrap();
        alice_to_mallory_tx
            .send((local_id, local_vv))
            .await
            .unwrap();

        bob_to_alice_rx.recv().await.unwrap();
    });

    // Bob is also a writer who first connects to Mallory and attempts to sync with her. That
    // should fail because Mallory tampered with the blocks. Bob then connects to Alice and syncs
    // with her which should succeed.
    env.actor("bob", async move {
        let (network, repo, _reg) = actor::setup().await;

        let expected_vv = mallory_to_bob_rx.recv().await.unwrap();

        // Connect to Mallory and wait until index is synced (blocks should be rejected).
        network.add_user_provided_peer(&actor::lookup_addr("mallory").await);

        info!("syncing with Mallory");

        common::eventually(&repo, || async {
            let local_vv = repo.local_branch().unwrap().version_vector().await.unwrap();
            local_vv == expected_vv
        })
        .await;

        // Now connect to Alice. Bob should reject the corrupted blocks from Mallory and download
        // them from Alice instead.
        network.add_user_provided_peer(&actor::lookup_addr("alice").await);

        info!("syncing with Alice");

        common::expect_file_content(&repo, "test.txt", content).await;

        bob_to_alice_tx.send(()).await.unwrap();
        bob_to_mallory_tx.send(()).await.unwrap();
    });

    // Mallory is the adversary. She is a blind replica so doesn't have access to the data but she
    // tries to corrupt the data by tampering with the block nonces.
    env.actor("mallory", async move {
        let network = actor::create_network(Proto::Tcp).await;
        let repo = actor::create_repo_with_mode(DEFAULT_REPO, AccessMode::Blind).await;
        let _reg = network.register(repo.handle()).await;

        // Connect to Alice and wait until fully synced (index + blocks).
        network.add_user_provided_peer(&actor::lookup_addr("alice").await);

        let (alice_id, alice_expected_vv) = alice_to_mallory_rx.recv().await.unwrap();

        common::eventually(&repo, || async {
            if !check_progress_full(&repo).await {
                return false;
            }

            let alice_actual_vv = repo.get_branch_version_vector(&alice_id).await.unwrap();
            alice_actual_vv == alice_expected_vv
        })
        .await;

        // Tamper with the nonces (need raw db access to do this).
        {
            use sqlx::{sqlite::SqliteConnection, Connection};

            info!("tampering with nonces");

            let mut conn =
                SqliteConnection::connect(actor::get_repo_path(DEFAULT_REPO).to_str().unwrap())
                    .await
                    .unwrap();

            sqlx::query("UPDATE blocks SET nonce = ?")
                .bind(b"******* pwned by mallory *******".as_ref())
                .execute(&mut conn)
                .await
                .unwrap();
        }

        mallory_to_bob_tx.send(alice_expected_vv).await.unwrap();
        bob_to_mallory_rx.recv().await.unwrap();
    });
}

async fn check_progress_full(repo: &Repository) -> bool {
    let progress = repo.sync_progress().await.unwrap();
    progress.total > 0 && progress.value == progress.total
}

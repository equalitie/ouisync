//! Garbage collection tests

mod common;

use self::common::{actor, Env, DEFAULT_REPO};
use ouisync::{File, BLOB_HEADER_SIZE, BLOCK_SIZE};
use tokio::sync::mpsc;
use tracing::Instrument;

#[test]
fn local_delete_local_file() {
    let mut env = Env::new();

    env.actor("local", async {
        let repo = actor::create_repo("test").await;

        assert_eq!(repo.count_blocks().await.unwrap(), 0);

        repo.create_file("test.dat").await.unwrap();

        // 1 block for the file + 1 block for the root directory
        assert_eq!(repo.count_blocks().await.unwrap(), 2);

        repo.remove_entry("test.dat").await.unwrap();
        repo.force_work().await.unwrap();

        // just 1 block for the root directory
        assert_eq!(repo.count_blocks().await.unwrap(), 1);
    });
}

#[test]
fn local_delete_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content = common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("remote", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            let mut file = repo.create_file("test.dat").await.unwrap();
            file.write(&content).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("local", {
        async move {
            let (network, repo, _reg) = actor::setup().await;
            let peer_addr = actor::lookup_addr("remote").await;
            network.add_user_provided_peer(&peer_addr);

            common::expect_file_content(&repo, "test.dat", &content).await;
            repo.force_work().await.unwrap();

            // 2 blocks for the file + 1 block for the remote root directory
            assert_eq!(repo.count_blocks().await.unwrap(), 3);

            repo.remove_entry("test.dat").await.unwrap();
            repo.force_work().await.unwrap();

            // Both the file and the remote root are deleted, only the local root remains to track the
            // tombstone.
            assert_eq!(repo.count_blocks().await.unwrap(), 1);

            tx.send(()).await.unwrap();
        }
    });
}

#[test]
fn remote_delete_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    env.actor("remote", async move {
        let (_network, repo, _reg) = actor::setup().await;

        repo.create_file("test.dat").await.unwrap();
        rx.recv().await.unwrap();

        repo.remove_entry("test.dat").await.unwrap();
        rx.recv().await.unwrap();
    });

    env.actor("local", async move {
        let (network, repo, _reg) = actor::setup().await;
        let peer_addr = actor::lookup_addr("remote").await;
        network.add_user_provided_peer(&peer_addr);

        common::expect_file_content(&repo, "test.dat", &[]).await;
        repo.force_work().await.unwrap();

        // 1 block for the file + 1 block for the remote root directory
        assert_eq!(repo.count_blocks().await.unwrap(), 2);
        tx.send(()).await.unwrap();

        common::expect_entry_not_found(&repo, "test.dat").await;
        repo.force_work().await.unwrap();

        // The remote file is removed but the remote root remains to track the tombstone
        assert_eq!(repo.count_blocks().await.unwrap(), 1);
        tx.send(()).await.unwrap();
    });
}

#[test]
fn local_truncate_local_file() {
    let mut env = Env::new();

    env.actor("local", async move {
        let repo = actor::create_repo(DEFAULT_REPO).await;

        let mut file = repo.create_file("test.dat").await.unwrap();
        write_to_file(&mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
        file.flush().await.unwrap();

        repo.force_work()
            .instrument(tracing::info_span!("force_work", step = 1))
            .await
            .unwrap();

        // 2 blocks for the file + 1 block for the root directory
        assert_eq!(repo.count_blocks().await.unwrap(), 3);

        file.truncate(0).await.unwrap();
        file.flush().await.unwrap();

        repo.force_work()
            .instrument(tracing::info_span!("force_work", step = 2))
            .await
            .unwrap();

        // 1 block for the file + 1 block for the root directory
        assert_eq!(repo.count_blocks().await.unwrap(), 2);
    });
}

#[test]
fn local_truncate_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content = common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("remote", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            let mut file = repo.create_file("test.dat").await.unwrap();
            file.write(&content).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("local", {
        async move {
            let (network, repo, _reg) = actor::setup().await;

            let peer_addr = actor::lookup_addr("remote").await;
            network.add_user_provided_peer(&peer_addr);

            common::expect_file_content(&repo, "test.dat", &content).await;
            repo.force_work().await.unwrap();

            // 2 blocks for the file + 1 block for the remote root directory
            assert_eq!(repo.count_blocks().await.unwrap(), 3);

            let mut file = repo.open_file("test.dat").await.unwrap();
            file.fork(repo.local_branch().unwrap()).await.unwrap();
            file.truncate(0).await.unwrap();
            file.flush().await.unwrap();

            repo.force_work().await.unwrap();

            //   1 block for the file (the original 2 blocks were removed)
            // + 1 block for the local root (created when the file was forked)
            assert_eq!(repo.count_blocks().await.unwrap(), 2);

            tx.send(()).await.unwrap();
        }
    });
}

#[test]
fn remote_truncate_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);
    let content = common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("remote", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            let mut file = repo.create_file("test.dat").await.unwrap();
            file.write(&content).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();

            file.truncate(0).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("local", {
        async move {
            let (network, repo, _reg) = actor::setup().await;

            let peer_addr = actor::lookup_addr("remote").await;
            network.add_user_provided_peer(&peer_addr);

            common::expect_file_content(&repo, "test.dat", &content).await;
            repo.force_work().await.unwrap();

            // 2 blocks for the file + 1 block for the remote root
            assert_eq!(repo.count_blocks().await.unwrap(), 3);
            tx.send(()).await.unwrap();

            common::expect_file_content(&repo, "test.dat", &[]).await;
            repo.force_work().await.unwrap();

            // 1 block for the file + 1 block for the remote root
            assert_eq!(repo.count_blocks().await.unwrap(), 2);
            tx.send(()).await.unwrap();
        }
    });
}

async fn write_to_file(file: &mut File, size: usize) {
    file.write(&common::random_content(size)).await.unwrap();
}

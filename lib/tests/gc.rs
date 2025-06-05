//! Garbage collection tests

#[macro_use]
mod common;

use self::common::{actor, Env, DEFAULT_REPO};
use common::Proto;
use ouisync::{AccessMode, File, Repository, BLOB_HEADER_SIZE, BLOCK_SIZE};
use tokio::sync::mpsc;

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

        // just 1 block for the root directory
        expect_block_count(&repo, 1).await;
    });
}

#[test]
fn local_delete_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("remote", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            let mut file = repo.create_file("test.dat").await.unwrap();
            file.write_all(&content).await.unwrap();
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

            // 2 blocks for the file + 1 block for the local root directory.
            expect_block_count(&repo, 3).await;

            repo.remove_entry("test.dat").await.unwrap();

            // 1 block for the local root directory, to track the tombstone.
            expect_block_count(&repo, 1).await;

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

        // 1 block for the file + 1 block for the local root directory.
        expect_block_count(&repo, 2).await;
        tx.send(()).await.unwrap();

        common::expect_entry_not_found(&repo, "test.dat").await;

        // 1 block for the local root directory.
        expect_block_count(&repo, 1).await;
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

        // 2 blocks for the file + 1 block for the root directory
        assert_eq!(repo.count_blocks().await.unwrap(), 3);

        file.truncate(0).unwrap();
        file.flush().await.unwrap();

        // 1 block for the file + 1 block for the root directory
        expect_block_count(&repo, 2).await;
    });
}

#[test]
fn local_truncate_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("remote", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            let mut file = repo.create_file("test.dat").await.unwrap();
            file.write_all(&content).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("local", {
        async move {
            let (network, repo, _reg) = actor::setup().await;

            let peer_addr = actor::lookup_addr("remote").await;
            network.add_user_provided_peer(&peer_addr);

            common::expect_local_file_content(&repo, "test.dat", &content).await;

            // 2 blocks for the file + 1 block for the local root directory.
            expect_block_count(&repo, 3).await;

            let mut file = repo.open_file("test.dat").await.unwrap();
            file.fork(repo.local_branch().unwrap()).await.unwrap();
            file.truncate(0).unwrap();
            file.flush().await.unwrap();

            //   1 block for the file (the original 2 blocks were removed)
            // + 1 block for the local root directory
            expect_block_count(&repo, 2).await;

            tx.send(()).await.unwrap();
        }
    });
}

#[test]
fn remote_truncate_remote_file() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);
    let content = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    env.actor("remote", {
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = actor::setup().await;

            let mut file = repo.create_file("test.dat").await.unwrap();
            file.write_all(&content).await.unwrap();
            file.flush().await.unwrap();

            rx.recv().await.unwrap();

            file.truncate(0).unwrap();
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

            // 2 blocks for the file + 1 block for the local root
            expect_block_count(&repo, 3).await;
            tx.send(()).await.unwrap();

            common::expect_file_content(&repo, "test.dat", &[]).await;

            // 1 block for the file + 1 block for the local root
            expect_block_count(&repo, 2).await;
            tx.send(()).await.unwrap();
        }
    });
}

// Test that garbage collection works correctly even when the repository is initially opened in
// blind mode and later changed to write mode.
#[test]
fn change_access_mode() {
    let mut env = Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let content1 = common::random_bytes(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);
    let content2 = common::random_bytes(3 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    async fn create_repo_and_reopen_in_blind_mode() -> Repository {
        let params = actor::get_repo_params(DEFAULT_REPO);
        let secrets = actor::get_repo_secrets(DEFAULT_REPO);
        let repo = Repository::create(
            &params,
            ouisync::Access::WriteUnlocked {
                secrets: secrets.write_secrets().unwrap().clone(),
            },
        )
        .await
        .unwrap();
        repo.close().await.unwrap();

        Repository::open(&params, None, AccessMode::Blind)
            .await
            .unwrap()
    }

    env.actor("remote", {
        let content1 = content1.clone();

        async move {
            let network = actor::create_network(Proto::Tcp).await;
            let repo = create_repo_and_reopen_in_blind_mode().await;

            repo.set_access_mode(AccessMode::Write, None).await.unwrap();

            let mut file = repo.create_file("1.dat").await.unwrap();
            file.write_all(&content1).await.unwrap();
            file.flush().await.unwrap();

            let _reg = network.register(repo.handle());

            rx.recv().await.unwrap();
        }
    });

    env.actor("local", {
        async move {
            let network = actor::create_network(Proto::Tcp).await;
            let repo = create_repo_and_reopen_in_blind_mode().await;

            let peer_addr = actor::lookup_addr("remote").await;
            network.add_user_provided_peer(&peer_addr);

            repo.set_access_mode(AccessMode::Write, None).await.unwrap();

            let mut file = repo.create_file("2.dat").await.unwrap();
            file.write_all(&content2).await.unwrap();
            file.flush().await.unwrap();
            drop(file);

            let _reg = network.register(repo.handle());

            // Ensure the remote file is completely reveived to avoid false positives in the
            // check that follows.
            common::expect_file_content(&repo, "1.dat", &content1).await;

            // 2 blocks for '1.dat', 3 blocks for '2.dat' and 1 blocks for the local root directory
            // (the remote root is removed when the remote branch is pruned after the branches are
            // merged).
            expect_block_count(&repo, 6).await;

            tx.send(()).await.unwrap();
        }
    });
}

async fn write_to_file(file: &mut File, size: usize) {
    file.write_all(&common::random_bytes(size)).await.unwrap();
}

async fn expect_block_count(repo: &Repository, expected: u64) {
    common::eventually(repo, || async {
        let actual = repo.count_blocks().await.unwrap();

        if actual == expected {
            true
        } else {
            warn!(actual, expected, "block count");
            false
        }
    })
    .await
}

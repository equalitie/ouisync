//! Synchronization tests

mod common;

use self::common::{new, old, NetworkExt, Proto};
use assert_matches::assert_matches;
use camino::Utf8Path;
use futures_util::future;
use ouisync::{
    Access, AccessMode, AccessSecrets, EntryType, Error, Repository, RepositoryDb, WriteSecrets,
    BLOB_HEADER_SIZE, BLOCK_SIZE,
};
use rand::Rng;
use std::{cmp::Ordering, io::SeekFrom, sync::Arc};
use tokio::sync::{broadcast, mpsc};
use tracing::{instrument, Instrument};

#[test]
fn relink_repository() {
    let mut env = new::Env::new();

    // Bidirectional side-channel
    let (writer_tx, mut writer_rx) = mpsc::channel(1);
    let (reader_tx, mut reader_rx) = mpsc::channel(1);

    let secrets = AccessSecrets::random_write();

    env.actor("writer", {
        let secrets = secrets.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            // Create the file
            let mut file = repo.create_file("test.txt").await.unwrap();
            file.write(b"first").await.unwrap();
            file.flush().await.unwrap();

            // Wait for the reader to see the file and then unlink its repo
            reader_rx.recv().await;

            // Modify the file while reader is unlinked
            file.truncate(0).await.unwrap();
            file.write(b"second").await.unwrap();
            file.flush().await.unwrap();

            // Notify the reader the file is updated
            writer_tx.send(()).await.unwrap();

            // Wait until reader is done
            reader_rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let (network, repo, reg) = common::setup_actor(secrets).await;
        network.connect("writer");

        // Wait until we see the original file
        common::expect_file_content(&repo, "test.txt", b"first").await;

        // Unlink the repo and notify the writer
        drop(reg);
        reader_tx.send(()).await.unwrap();

        // Wait until writer is done updating the file
        writer_rx.recv().await;

        // Relink the repo
        let _reg = network.handle().register(repo.store().clone());

        // Wait until the file is updated
        common::expect_file_content(&repo, "test.txt", b"second").await;

        // Notify the writer that we are done
        reader_tx.send(()).await.unwrap();
    });
}

#[test]
fn remove_remote_file() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let secrets = AccessSecrets::random_write();

    env.actor("creator", {
        let secrets = secrets.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            let mut file = repo.create_file("test.txt").await.unwrap();
            file.flush().await.unwrap();
            drop(file);

            expect_entry_not_found(&repo, "test.txt").await;

            tx.send(()).await.unwrap();
        }
    });

    env.actor("remover", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;
        network.connect("creator");

        common::expect_file_content(&repo, "test.txt", &[]).await;

        repo.remove_entry("test.txt").await.unwrap();

        rx.recv().await;
    });
}

#[test]
fn relay_write() {
    // There used to be a deadlock that got triggered only when transferring a sufficiently large
    // file.
    let file_size = 4 * 1024 * 1024;
    relay_case(Proto::Tcp, file_size, AccessMode::Write)
}

#[test]
fn relay_blind() {
    let file_size = 4 * 1024 * 1024;
    relay_case(Proto::Tcp, file_size, AccessMode::Blind)
}

// Simulate two peers that can't connect to each other but both can connect to a third ("relay")
// peer.
fn relay_case(_proto: Proto, file_size: usize, relay_access_mode: AccessMode) {
    let mut env = new::Env::new();
    let (tx, _) = broadcast::channel(1);

    let secrets = AccessSecrets::random_write();
    let content = Arc::new(common::random_content(file_size));

    env.actor("relay", {
        let secrets = secrets.with_mode(relay_access_mode);
        let mut rx = tx.subscribe();

        async move {
            let network = common::create_network().await;
            let (_repo, _reg) = common::create_linked_repo(secrets, &network).await;

            rx.recv().await.unwrap();
        }
    });

    env.actor("writer", {
        let secrets = secrets.clone();
        let content = content.clone();
        let mut rx = tx.subscribe();

        async move {
            let (network, repo, _reg) = common::setup_actor(secrets).await;

            network.connect("relay");

            let mut file = repo.create_file("test.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content, 4096).await;
            file.flush().await.unwrap();

            rx.recv().await.unwrap();
        }
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;

        network.connect("relay");

        common::expect_file_content(&repo, "test.dat", &content).await;

        tx.send(()).unwrap();
    });
}

#[test]
fn transfer_large_file() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1); // side-channel

    let secrets = AccessSecrets::random_write();

    let file_size = 4 * 1024 * 1024;
    let content = Arc::new(common::random_content(file_size));

    env.actor("writer", {
        let secrets = secrets.clone();
        let content = content.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            let mut file = repo.create_file("test.dat").await.unwrap();
            common::write_in_chunks(&mut file, &content, 4096).await;
            file.flush().await.unwrap();

            rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;
        network.connect("writer");

        common::expect_file_content(&repo, "test.dat", &content).await;

        tx.send(()).await.unwrap();
    });
}

#[test]
fn transfer_many_files() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1);

    let num_files = 10;
    let min_size = 0;
    let max_size = 128 * 1024;
    let files = {
        let mut rng = rand::thread_rng();
        let files: Vec<_> = (0..num_files)
            .map(|_| {
                let size = rng.gen_range(min_size..max_size);
                let mut buffer = vec![0; size];
                rng.fill(&mut buffer[..]);
                buffer
            })
            .collect();
        Arc::new(files)
    };

    let secrets = AccessSecrets::random_write();

    env.actor("writer", {
        let secrets = secrets.clone();
        let files = files.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            for (index, content) in files.iter().enumerate() {
                let name = format!("file-{}.dat", index);
                let mut file = repo.create_file(&name).await.unwrap();
                common::write_in_chunks(&mut file, content, 4096).await;
                file.flush().await.unwrap();
            }

            rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;
        network.connect("writer");

        for (index, content) in files.iter().enumerate() {
            let name = format!("file-{}.dat", index);
            common::expect_file_content(&repo, &name, content).await;
        }

        tx.send(()).await.unwrap();
    });
}

// Test for an edge case where a sync happens while we are in the middle of writing a file.
// This test makes sure that when the sync happens, the partially written file content is not
// garbage collected prematurelly.
#[cfg(not(feature = "simulation"))]
#[tokio::test(flavor = "multi_thread")]
async fn sync_during_file_write() {
    let mut env = old::Env::new();

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.handle().register(repo_a.store().clone());
    let _reg_b = node_b.handle().register(repo_b.store().clone());

    let id_b = *repo_b.local_branch().unwrap().id();

    let content = common::random_content(3 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    // A: Create empty file
    let mut file_a = repo_a.create_file("foo.txt").await.unwrap();

    // B: Wait until everything gets merged
    common::expect_file_version_content(&repo_b, "foo.txt", Some(&id_b), &[]).await;

    // A: Write half of the file content but don't flush yet.
    common::write_in_chunks(&mut file_a, &content[..content.len() / 2], 4096).await;

    // B: Write a file. Excluding the unflushed changes by A, this makes B's branch newer than
    // A's.
    let mut file_b = repo_b.create_file("bar.txt").await.unwrap();
    file_b.write(b"bar").await.unwrap();
    file_b.flush().await.unwrap();
    drop(file_b);

    // A: Wait until we see the file created by B
    common::expect_file_content(&repo_a, "bar.txt", b"bar").await;

    // A: Write the second half of the content and flush.
    common::write_in_chunks(&mut file_a, &content[content.len() / 2..], 4096).await;
    file_a.flush().await.unwrap();

    // A: Reopen the file and verify it has the expected full content
    let mut file_a = repo_a.open_file("foo.txt").await.unwrap();
    let actual_content = file_a.read_to_end().await.unwrap();
    assert_eq!(actual_content, content);

    // B: Wait until we see the file as well
    common::expect_file_content(&repo_b, "foo.txt", &content).await;
}

#[cfg(not(feature = "simulation"))]
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_modify_open_file() {
    let mut env = old::Env::new();

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.handle().register(repo_a.store().clone());
    let _reg_b = node_b.handle().register(repo_b.store().clone());

    let id_a = *repo_a.local_branch().unwrap().id();
    let id_b = *repo_b.local_branch().unwrap().id();

    let content_a = common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);
    let content_b = common::random_content(2 * BLOCK_SIZE - BLOB_HEADER_SIZE);

    // A: Create empty file
    let mut file_a = repo_a.create_file("file.txt").await.unwrap();

    // B: Wait until everything gets merged
    common::expect_file_version_content(&repo_b, "file.txt", Some(&id_b), &[]).await;

    // A: Write to file but don't flush yet
    common::write_in_chunks(&mut file_a, &content_a, 4096)
        .instrument(tracing::info_span!("write file.txt"))
        .instrument(repo_a.span())
        .await;

    // B: Write to the same file and flush
    async {
        let mut file_b = repo_b.open_file("file.txt").await.unwrap();
        common::write_in_chunks(&mut file_b, &content_b, 4096)
            .instrument(tracing::info_span!("write file.txt"))
            .await;
        file_b
            .flush()
            .instrument(tracing::info_span!("flush file.txt"))
            .await
            .unwrap();
    }
    .instrument(repo_b.span())
    .await;

    // A: Wait until we see B's writes
    common::expect_file_version_content(&repo_a, "file.txt", Some(&id_b), &content_b).await;

    // A: Flush the file
    file_a
        .flush()
        .instrument(tracing::info_span!("flush file.txt"))
        .instrument(repo_a.span())
        .await
        .unwrap();
    drop(file_a);

    // A: Verify both versions of the file are still present
    assert_matches!(
        repo_a.open_file("file.txt").await,
        Err(Error::AmbiguousEntry)
    );

    let mut file_a = repo_a.open_file_version("file.txt", &id_a).await.unwrap();
    let actual_content_a = file_a.read_to_end().await.unwrap();

    let mut file_b = repo_a.open_file_version("file.txt", &id_b).await.unwrap();
    let actual_content_b = file_b.read_to_end().await.unwrap();

    assert!(actual_content_a == content_a);
    assert!(actual_content_b == content_b);
}

// Test that the local version changes monotonically even when the local branch temporarily becomes
// outdated.
#[cfg(not(feature = "simulation"))]
#[tokio::test(flavor = "multi_thread")]
async fn recreate_local_branch() {
    let mut env = old::Env::new();

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;

    let store_a = env.next_store();
    let device_id_a = rand::random();
    let write_secrets = WriteSecrets::random();
    let repo_a = Repository::create(
        RepositoryDb::create(&store_a).await.unwrap(),
        device_id_a,
        Access::WriteUnlocked {
            secrets: write_secrets.clone(),
        },
    )
    .await
    .unwrap();

    let repo_b = env
        .create_repo_with_secrets(AccessSecrets::Write(write_secrets.clone()))
        .await;

    let id_b = *repo_b.local_branch().unwrap().id();

    let mut file = repo_a.create_file("foo.txt").await.unwrap();
    file.write(b"hello from A\n").await.unwrap();
    file.flush().await.unwrap();

    let vv_a_0 = repo_a
        .local_branch()
        .unwrap()
        .version_vector()
        .await
        .unwrap();

    drop(file);

    // A: Reopen the repo in read mode to disable merger
    repo_a.close().await.unwrap();
    drop(repo_a);

    let repo_a = Repository::open_with_mode(&store_a, device_id_a, None, AccessMode::Read)
        .await
        .unwrap();

    // A + B: establish link
    let reg_a = node_a.handle().register(repo_a.store().clone());
    let reg_b = node_b.handle().register(repo_b.store().clone());

    // B: Sync with A
    common::expect_file_version_content(&repo_b, "foo.txt", Some(&id_b), b"hello from A\n").await;

    // B: Modify the repo. This makes B's branch newer than A's
    let mut file = repo_b.open_file("foo.txt").await.unwrap();
    file.seek(SeekFrom::End(0)).await.unwrap();
    file.write(b"hello from B\n").await.unwrap();
    file.flush().await.unwrap();

    let vv_b = repo_b
        .local_branch()
        .unwrap()
        .version_vector()
        .await
        .unwrap();

    drop(file);

    assert!(vv_b > vv_a_0);

    // A: Sync with B. Afterwards our local branch will become outdated compared to B's
    common::expect_file_content(&repo_a, "foo.txt", b"hello from A\nhello from B\n").await;

    // A: Reopen in write mode
    drop(reg_a);
    drop(reg_b);

    repo_a.close().await.unwrap();
    drop(repo_a);

    let repo_a = Repository::open(&store_a, device_id_a, None).await.unwrap();

    // A: Modify the repo
    repo_a.create_file("bar.txt").await.unwrap();
    repo_a.force_work().await.unwrap();

    // A: Make sure the local version changed monotonically.
    let vv_a_1 = repo_a
        .local_branch()
        .unwrap()
        .version_vector()
        .await
        .unwrap();

    assert_eq!(
        vv_a_1.partial_cmp(&vv_b),
        Some(Ordering::Greater),
        "non-monotonic version progression"
    );
}

#[test]
fn transfer_directory_with_file() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1); // side-channel

    let secrets = AccessSecrets::random_write();

    env.actor("writer", {
        let secrets = secrets.clone();

        async move {
            let network = common::create_network().await;
            let repo = common::create_repo(secrets).await;
            let _reg = network.handle().register(repo.store().clone());

            let mut dir = repo.create_directory("food").await.unwrap();
            dir.create_file("pizza.jpg".into()).await.unwrap();

            rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let network = common::create_network().await;
        let repo = common::create_repo(secrets).await;
        let _reg = network.handle().register(repo.store().clone());

        network.connect("writer");

        common::expect_entry_exists(&repo, "food/pizza.jpg", EntryType::File).await;

        tx.send(()).await.unwrap();
    });
}

#[test]
fn transfer_directory_with_subdirectory() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1);
    let secrets = AccessSecrets::random_write();

    env.actor("writer", {
        let secrets = secrets.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            let mut dir = repo.create_directory("food").await.unwrap();
            dir.create_directory("mediterranean".into()).await.unwrap();

            rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;
        network.connect("writer");

        common::expect_entry_exists(&repo, "food/mediterranean", EntryType::Directory).await;

        tx.send(()).await.unwrap();
    });
}

#[test]
fn remote_rename_file() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1);
    let secrets = AccessSecrets::random_write();

    env.actor("writer", {
        let secrets = secrets.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            // Create the file and wait until reader has seen it
            repo.create_file("foo.txt").await.unwrap();
            rx.recv().await;

            // Rename it and wait until reader is done
            repo.move_entry("/", "foo.txt", "/", "bar.txt")
                .await
                .unwrap();
            rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;
        network.connect("writer");

        common::expect_entry_exists(&repo, "foo.txt", EntryType::File).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "bar.txt", EntryType::File).await;
        expect_entry_not_found(&repo, "foo.txt").await;
        tx.send(()).await.unwrap();
    });
}

#[test]
fn remote_rename_empty_directory() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1);
    let secrets = AccessSecrets::random_write();

    env.actor("writer", {
        let secrets = secrets.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            // Create directory and wait until reader has seen it
            repo.create_directory("foo").await.unwrap();
            rx.recv().await;

            // Rename the directory and wait until reader is done
            repo.move_entry("/", "foo", "/", "bar").await.unwrap();
            rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;
        network.connect("writer");

        common::expect_entry_exists(&repo, "foo", EntryType::Directory).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "bar", EntryType::Directory).await;
        expect_entry_not_found(&repo, "foo").await;
        tx.send(()).await.unwrap();
    });
}

#[test]
fn remote_rename_non_empty_directory() {
    let mut env = new::Env::new();
    let (tx, mut rx) = mpsc::channel(1);
    let secrets = AccessSecrets::random_write();

    env.actor("writer", {
        let secrets = secrets.clone();

        async move {
            let (_network, repo, _reg) = common::setup_actor(secrets).await;

            // Create a directory with content and wait until reader has seen it
            let mut dir = repo.create_directory("foo").await.unwrap();
            dir.create_file("data.txt".into()).await.unwrap();
            rx.recv().await;

            // Rename the directory and wait until reader is done
            repo.move_entry("/", "foo", "/", "bar").await.unwrap();
            rx.recv().await;
        }
    });

    env.actor("reader", async move {
        let (network, repo, _reg) = common::setup_actor(secrets).await;
        network.connect("writer");

        common::expect_entry_exists(&repo, "foo/data.txt", EntryType::File).await;
        tx.send(()).await.unwrap();

        common::expect_entry_exists(&repo, "bar/data.txt", EntryType::File).await;
        expect_entry_not_found(&repo, "foo").await;
        tx.send(()).await.unwrap();
    });
}

#[cfg(not(feature = "simulation"))]
#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_directory_during_conflict() {
    let mut env = old::Env::new();

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.handle().register(repo_a.store().clone());
    let _reg_b = node_b.handle().register(repo_b.store().clone());

    // Create a conflict so the branches stay concurrent. This prevents the remote branch from
    // being pruned.
    repo_a.create_file("dummy.txt").await.unwrap();
    repo_b.create_file("dummy.txt").await.unwrap();

    repo_b.create_directory("foo").await.unwrap();
    expect_local_directory_exists(&repo_a, "foo").await;

    repo_b.move_entry("/", "foo", "/", "bar").await.unwrap();
    common::expect_entry_exists(&repo_a, "bar", EntryType::Directory).await;
    expect_entry_not_found(&repo_a, "foo").await;
}

#[cfg(not(feature = "simulation"))]
#[tokio::test(flavor = "multi_thread")]
async fn remote_move_file_to_directory_then_rename_that_directory() {
    let mut env = old::Env::new();

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.handle().register(repo_a.store().clone());
    let _reg_b = node_b.handle().register(repo_b.store().clone());

    repo_b.create_file("data.txt").await.unwrap();

    common::expect_entry_exists(&repo_a, "data.txt", EntryType::File).await;

    repo_b.create_directory("archive").await.unwrap();
    repo_b
        .move_entry("/", "data.txt", "archive", "data.txt")
        .await
        .unwrap();

    common::expect_entry_exists(&repo_a, "archive/data.txt", EntryType::File).await;

    repo_b
        .move_entry("/", "archive", "/", "trash")
        .await
        .unwrap();

    common::expect_entry_exists(&repo_a, "trash/data.txt", EntryType::File).await;
    expect_entry_not_found(&repo_a, "data.txt").await;
    expect_entry_not_found(&repo_a, "archive").await;
}

#[cfg(not(feature = "simulation"))]
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_update_and_delete_during_conflict() {
    let mut env = old::Env::new();

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let reg_a = node_a.handle().register(repo_a.store().clone());
    let reg_b = node_b.handle().register(repo_b.store().clone());

    let id_a = *repo_a.local_branch().unwrap().id();
    let id_b = *repo_b.local_branch().unwrap().id();

    // Create a conflict so the branches stay concurrent. This prevents the remote branch from
    // being pruned.
    repo_a.create_file("dummy.txt").await.unwrap();
    repo_b.create_file("dummy.txt").await.unwrap();

    let mut file = repo_b.create_file("data.txt").await.unwrap();
    let mut content = vec![0u8; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE]; // exactly 2 blocks
    rand::thread_rng().fill(&mut content[..]);
    file.write(&content).await.unwrap();
    file.flush().await.unwrap();

    common::expect_file_version_content(&repo_a, "data.txt", Some(&id_a), &content).await;

    // Disconnect to allow concurrent operations on the file
    drop(reg_a);
    drop(reg_b);

    // A removes it (this garbage-collects its blocks)
    repo_a.remove_entry("data.txt").await.unwrap();

    // B writes to it in such a way that the first block remains unchanged
    let chunk = {
        let offset = content.len() - 64;
        &mut content[offset..]
    };

    rand::thread_rng().fill(chunk);

    file.seek(SeekFrom::End(-(chunk.len() as i64)))
        .await
        .unwrap();
    file.write(chunk).await.unwrap();
    file.flush().await.unwrap();

    // Reconnect
    let _reg_a = node_a.handle().register(repo_a.store().clone());
    let _reg_b = node_b.handle().register(repo_b.store().clone());

    // A is able to read the whole file again including the previously gc-ed blocks.
    common::expect_file_version_content(&repo_a, "data.txt", Some(&id_b), &content).await;
}

// https://github.com/equalitie/ouisync/issues/86
#[cfg(not(feature = "simulation"))]
#[tokio::test(flavor = "multi_thread")]
async fn content_stays_available_during_sync() {
    let mut env = old::Env::new();

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;

    let repo_a = env.create_repo().await;

    // B is read-only to disable the merger which could otherwise interfere with this test.
    let repo_b = env
        .create_repo_with_secrets(repo_a.secrets().with_mode(AccessMode::Read))
        .await;

    let _reg_a = node_a.handle().register(repo_a.store().clone());
    let _reg_b = node_b.handle().register(repo_b.store().clone());

    let mut content0 = vec![0; 32];
    rand::thread_rng().fill(&mut content0[..]);

    let mut content1 = vec![0; 128 * 1024];
    rand::thread_rng().fill(&mut content1[..]);

    // First create and sync "b/c.dat"
    async {
        let mut file = repo_a.create_file("b/c.dat").await.unwrap();
        file.write(&content0).await.unwrap();
        file.flush().await.unwrap();
        tracing::info!("done");
    }
    .instrument(tracing::info_span!("create b/c.dat"))
    .await;

    common::expect_file_content(&repo_b, "b/c.dat", &content0).await;

    // Then create "a.dat" and "b/d.dat". Because the entries are processed in lexicographical
    // order, the blocks of "a.dat" are requested before the blocks of "b". Thus there is a period
    // of time after the snapshot is completed but before the blocks of "b" are downloaded during
    // which the latest version of "b" can't be opened because its blocks haven't been downloaded
    // yet (because the blocks of "a.dat" must be downloaded first). This test verifies that the
    // previous content of "b" can still be accessed even during this period.

    let task_a = async {
        async {
            let mut file = repo_a.create_file("a.dat").await.unwrap();
            file.write(&content1).await.unwrap();
            file.flush().await.unwrap();
            tracing::info_span!("done");
        }
        .instrument(tracing::info_span!("create a.dat"))
        .await;

        async {
            repo_a.create_file("b/d.dat").await.unwrap();
            tracing::info_span!("done");
        }
        .instrument(tracing::info_span!("create b/d.dat"))
        .await;
    };

    let task_b = common::eventually(&repo_b, || async {
        // The first file stays available at all times...
        assert!(common::check_file_version_content(&repo_b, "b/c.dat", None, &content0).await);

        // ...until the new files are transfered
        for (name, content) in [("a.dat", &content1[..]), ("b/d.dat", &[])] {
            if !common::check_file_version_content(&repo_b, name, None, content).await {
                return false;
            }
        }

        true
    });

    future::join(task_a, task_b).await;
}

#[instrument]
async fn expect_entry_not_found(repo: &Repository, path: &str) {
    let path = Utf8Path::new(path);
    let name = path.file_name().unwrap();
    let parent = path.parent().unwrap();

    common::eventually(repo, || async {
        let parent = repo.open_directory(parent).await.unwrap();

        match parent.lookup_unique(name) {
            Ok(_) => false,
            Err(Error::EntryNotFound) => true,
            Err(error) => panic!("unexpected error: {:?}", error),
        }
    })
    .await
}

#[instrument]
async fn expect_local_directory_exists(repo: &Repository, path: &str) {
    common::eventually(repo, || async {
        match repo.open_directory(path).await {
            Ok(dir) => dir.has_local_version(),
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => false,
            Err(error) => panic!("unexpected error: {:?}", error),
        }
    })
    .await
}

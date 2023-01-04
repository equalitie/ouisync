//! Synchronization tests

mod common;

use self::common::{Env, Proto};
use assert_matches::assert_matches;
use camino::Utf8Path;
use futures_util::future;
use ouisync::{
    Access, AccessMode, AccessSecrets, EntryType, Error, Repository, RepositoryDb, WriteSecrets,
    BLOB_HEADER_SIZE, BLOCK_SIZE,
};
use rand::Rng;
use std::{cmp::Ordering, io::SeekFrom, net::Ipv4Addr, sync::Arc};
use tokio::task;
use tracing::{instrument, Instrument, Span};

#[tokio::test(flavor = "multi_thread")]
async fn relink_repository() {
    let mut env = Env::with_seed(0);

    // Create two peers and connect them together.
    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;

    let (repo_a, repo_b) = env.create_linked_repos().await;

    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let reg_b = node_b.network.handle().register(repo_b.store().clone());

    // Create a file by A
    let mut file_a = async {
        let mut file_a = repo_a.create_file("test.txt").await.unwrap();
        file_a.write(b"first").await.unwrap();
        file_a.flush().await.unwrap();
        file_a
    }
    .instrument(repo_a.span())
    .await;

    // Wait until the file is seen by B
    common::expect_file_content(&repo_b, "test.txt", b"first").await;

    // Unlink B's repo
    tracing::info!("B: unlink");
    drop(reg_b);

    // Update the file while B's repo is unlinked
    async {
        file_a.truncate(0).await.unwrap();
        file_a.write(b"second").await.unwrap();
        file_a.flush().await.unwrap();
    }
    .instrument(repo_a.span())
    .await;

    // Re-register B's repo
    tracing::info!("B: relink");
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    // Wait until the file is updated
    common::expect_file_content(&repo_b, "test.txt", b"second").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_remote_file() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    // Create a file by A and wait until B sees it.
    let mut file = repo_a.create_file("test.txt").await.unwrap();
    file.flush().await.unwrap();

    common::expect_file_content(&repo_b, "test.txt", &[]).await;

    // Delete the file by B
    repo_b.remove_entry("test.txt").await.unwrap();

    // TODO: wait until A sees the file being deleted
}

#[tokio::test(flavor = "multi_thread")]
async fn relay_write() {
    // There used to be a deadlock that got triggered only when transferring a sufficiently large
    // file.
    let file_size = 4 * 1024 * 1024;
    relay_case(Proto::Tcp, file_size, AccessMode::Write).await
}

#[tokio::test(flavor = "multi_thread")]
async fn relay_blind() {
    let file_size = 4 * 1024 * 1024;
    relay_case(Proto::Tcp, file_size, AccessMode::Blind).await
}

// Simulate two peers that can't connect to each other but both can connect to a third ("relay")
// peer.
async fn relay_case(proto: Proto, file_size: usize, relay_access_mode: AccessMode) {
    let mut env = Env::with_seed(0);

    let node_a = env.create_node(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;
    let repo_a = env.create_repo().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());

    let node_b = env.create_node(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;
    let repo_b = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    // The "relay" peer.
    let node_r = env.create_node(proto.wrap((Ipv4Addr::LOCALHOST, 0))).await;
    let repo_r = env
        .create_repo_with_secrets(repo_a.secrets().with_mode(relay_access_mode))
        .await;
    let _reg_r = node_r.network.handle().register(repo_r.store().clone());

    // Connect A and B to the relay only
    node_a
        .network
        .add_user_provided_peer(&proto.listener_local_addr_v4(&node_r.network));
    node_b
        .network
        .add_user_provided_peer(&proto.listener_local_addr_v4(&node_r.network));

    let mut content = vec![0; file_size];
    env.rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it. The file must pass through R because A and B
    // are not connected to each other.
    tracing::info!("writing");
    async {
        let mut file = repo_a.create_file("test.dat").await.unwrap();
        common::write_in_chunks(&mut file, &content, 4096).await;
        file.flush().await.unwrap();
    }
    .instrument(repo_a.span())
    .await;

    tracing::info!("reading");
    common::expect_file_content(&repo_b, "test.dat", &content).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_large_file() {
    let file_size = 4 * 1024 * 1024;

    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let mut content = vec![0; file_size];
    env.rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it.
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    common::write_in_chunks(&mut file, &content, 4096).await;
    file.flush().await.unwrap();
    drop(file);

    common::expect_file_content(&repo_b, "test.dat", &content).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_multiple_files_sequentially() {
    let mut env = Env::with_seed(0);
    let file_sizes = [512 * 1024, 1024];

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let contents: Vec<_> = file_sizes
        .iter()
        .map(|size| {
            let mut content = vec![0; *size];
            env.rng.fill(&mut content[..]);
            content
        })
        .collect();

    for (index, content) in contents.iter().enumerate() {
        let name = format!("file-{}.dat", index);
        let mut file = repo_a.create_file(&name).await.unwrap();
        common::write_in_chunks(&mut file, content, 4096).await;
        file.flush().await.unwrap();
        tracing::info!("put {:?}", name);
        drop(file);

        // Wait until we see all the already transfered files
        for (index, content) in contents.iter().take(index + 1).enumerate() {
            let name = format!("file-{}.dat", index);
            common::expect_file_content(&repo_b, &name, content).await;
        }
    }
}

// Test for an edge case where a sync happens while we are in the middle of writing a file.
// This test makes sure that when the sync happens, the partially written file content is not
// garbage collected prematurelly.
#[tokio::test(flavor = "multi_thread")]
async fn sync_during_file_write() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let id_b = *repo_b.local_branch().unwrap().id();

    let mut content = vec![0; 3 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content[..]);

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

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_modify_open_file() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let id_a = *repo_a.local_branch().unwrap().id();
    let id_b = *repo_b.local_branch().unwrap().id();

    let mut content_a = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    let mut content_b = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content_a[..]);
    env.rng.fill(&mut content_b[..]);

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
#[tokio::test(flavor = "multi_thread")]
async fn recreate_local_branch() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;

    let store_a = env.next_store();
    let device_id_a = env.rng.gen();
    let write_secrets = WriteSecrets::generate(&mut env.rng);
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
    let reg_a = node_a.network.handle().register(repo_a.store().clone());
    let reg_b = node_b.network.handle().register(repo_b.store().clone());

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

#[tokio::test(flavor = "multi_thread")]
async fn transfer_many_files() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Quic).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let num_files = 10;
    let min_size = 0;
    let max_size = 128 * 1024;

    let contents: Vec<_> = (0..num_files)
        .map(|_| {
            let size = env.rng.gen_range(min_size..max_size);
            let mut buffer = vec![0; size];
            env.rng.fill(&mut buffer[..]);
            buffer
        })
        .collect();
    let contents = Arc::new(contents);

    let task_a = task::spawn({
        let contents = contents.clone();
        async move {
            for (index, content) in contents.iter().enumerate() {
                let name = format!("file-{}.dat", index);
                let mut file = repo_a.create_file(&name).await.unwrap();

                common::write_in_chunks(&mut file, content, 4096).await;
                file.flush().await.unwrap();

                tracing::info!("put {:?}", name);
            }
        }
        .instrument(Span::current())
    });

    let task_b = task::spawn(
        async move {
            common::eventually(&repo_b, || async {
                for (index, content) in contents.iter().enumerate() {
                    let name = format!("file-{}.dat", index);

                    if !common::check_file_version_content(&repo_b, &name, None, content).await {
                        return false;
                    }

                    tracing::info!("get {:?}", name);
                }

                true
            })
            .await
        }
        .instrument(Span::current()),
    );

    task_a.await.unwrap();
    task_b.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_directory_with_file() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let mut dir = repo_a.create_directory("food").await.unwrap();
    dir.create_file("pizza.jpg".into()).await.unwrap();

    expect_entry_exists(&repo_b, "food/pizza.jpg", EntryType::File).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_directory_with_subdirectory() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let mut dir0 = repo_a.create_directory("food").await.unwrap();
    dir0.create_directory("mediterranean".into()).await.unwrap();

    expect_entry_exists(&repo_b, "food/mediterranean", EntryType::Directory).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_file() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    repo_b.create_file("foo.txt").await.unwrap();

    // Wait until the file is synced and merged over to the local branch.
    expect_entry_exists(&repo_a, "foo.txt", EntryType::File).await;

    repo_b
        .move_entry("/", "foo.txt", "/", "bar.txt")
        .await
        .unwrap();

    expect_entry_exists(&repo_a, "bar.txt", EntryType::File).await;
    expect_entry_not_found(&repo_a, "foo.txt").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_empty_directory() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    repo_b.create_directory("foo").await.unwrap();

    expect_entry_exists(&repo_a, "foo", EntryType::Directory).await;

    repo_b.move_entry("/", "foo", "/", "bar").await.unwrap();

    expect_entry_exists(&repo_a, "bar", EntryType::Directory).await;
    expect_entry_not_found(&repo_a, "foo").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_non_empty_directory() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let mut dir = repo_b.create_directory("foo").await.unwrap();
    dir.create_file("data.txt".into()).await.unwrap();

    expect_entry_exists(&repo_a, "foo/data.txt", EntryType::File).await;

    repo_b.move_entry("/", "foo", "/", "bar").await.unwrap();

    expect_entry_exists(&repo_a, "bar/data.txt", EntryType::File).await;
    expect_entry_not_found(&repo_a, "foo").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_directory_during_conflict() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    // Create a conflict so the branches stay concurrent. This prevents the remote branch from
    // being pruned.
    repo_a.create_file("dummy.txt").await.unwrap();
    repo_b.create_file("dummy.txt").await.unwrap();

    repo_b.create_directory("foo").await.unwrap();
    expect_local_directory_exists(&repo_a, "foo").await;

    repo_b.move_entry("/", "foo", "/", "bar").await.unwrap();
    expect_entry_exists(&repo_a, "bar", EntryType::Directory).await;
    expect_entry_not_found(&repo_a, "foo").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_move_file_to_directory_then_rename_that_directory() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    repo_b.create_file("data.txt").await.unwrap();

    expect_entry_exists(&repo_a, "data.txt", EntryType::File).await;

    repo_b.create_directory("archive").await.unwrap();
    repo_b
        .move_entry("/", "data.txt", "archive", "data.txt")
        .await
        .unwrap();

    expect_entry_exists(&repo_a, "archive/data.txt", EntryType::File).await;

    repo_b
        .move_entry("/", "archive", "/", "trash")
        .await
        .unwrap();

    expect_entry_exists(&repo_a, "trash/data.txt", EntryType::File).await;
    expect_entry_not_found(&repo_a, "data.txt").await;
    expect_entry_not_found(&repo_a, "archive").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_update_and_delete_during_conflict() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let reg_a = node_a.network.handle().register(repo_a.store().clone());
    let reg_b = node_b.network.handle().register(repo_b.store().clone());

    let id_a = *repo_a.local_branch().unwrap().id();
    let id_b = *repo_b.local_branch().unwrap().id();

    // Create a conflict so the branches stay concurrent. This prevents the remote branch from
    // being pruned.
    repo_a.create_file("dummy.txt").await.unwrap();
    repo_b.create_file("dummy.txt").await.unwrap();

    let mut file = repo_b.create_file("data.txt").await.unwrap();
    let mut content = vec![0u8; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE]; // exactly 2 blocks
    env.rng.fill(&mut content[..]);
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

    env.rng.fill(chunk);

    file.seek(SeekFrom::End(-(chunk.len() as i64)))
        .await
        .unwrap();
    file.write(chunk).await.unwrap();
    file.flush().await.unwrap();

    // Reconnect
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    // A is able to read the whole file again including the previously gc-ed blocks.
    common::expect_file_version_content(&repo_a, "data.txt", Some(&id_b), &content).await;
}

// https://github.com/equalitie/ouisync/issues/86
#[tokio::test(flavor = "multi_thread")]
async fn content_stays_available_during_sync() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;

    let repo_a = env.create_repo().await;

    // B is read-only to disable the merger which could otherwise interfere with this test.
    let repo_b = env
        .create_repo_with_secrets(repo_a.secrets().with_mode(AccessMode::Read))
        .await;

    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let mut content0 = vec![0; 32];
    env.rng.fill(&mut content0[..]);

    let mut content1 = vec![0; 128 * 1024];
    env.rng.fill(&mut content1[..]);

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
async fn expect_entry_exists(repo: &Repository, path: &str, entry_type: EntryType) {
    common::eventually(repo, || check_entry_exists(repo, path, entry_type)).await
}

async fn check_entry_exists(repo: &Repository, path: &str, entry_type: EntryType) -> bool {
    let result = match entry_type {
        EntryType::File => repo.open_file(path).await.map(|_| ()),
        EntryType::Directory => repo.open_directory(path).await.map(|_| ()),
    };

    match result {
        Ok(()) => true,
        Err(Error::EntryNotFound | Error::BlockNotFound(_)) => false,
        Err(error) => panic!("unexpected error: {:?}", error),
    }
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

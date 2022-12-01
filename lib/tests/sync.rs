//! Synchronization tests

mod common;

use self::common::{Env, Proto};
use assert_matches::assert_matches;
use camino::Utf8Path;
use ouisync::{
    crypto::sign::PublicKey, Access, AccessMode, AccessSecrets, EntryType, Error, File, Repository,
    RepositoryDb, WriteSecrets, BLOB_HEADER_SIZE, BLOCK_SIZE,
};
use rand::Rng;
use std::{cmp::Ordering, io::SeekFrom, net::Ipv4Addr, sync::Arc};
use tokio::task;
use tracing::{Instrument, Span};

#[tokio::test(flavor = "multi_thread")]
async fn relink_repository() {
    let mut env = Env::with_seed(0);

    // Create two peers and connect them together.
    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;

    let (repo_a, repo_b) = env.create_linked_repos().await;

    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let reg_b = node_b.network.handle().register(repo_b.store().clone());

    // Create a file by A
    let mut file_a = repo_a.create_file("test.txt").await.unwrap();
    file_a.write(b"first").await.unwrap();
    file_a.flush().await.unwrap();

    // Wait until the file is seen by B
    expect_file_content(&repo_b, "test.txt", b"first").await;

    // Unlink B's repo
    drop(reg_b);

    // Update the file while B's repo is unlinked
    file_a.truncate(0).await.unwrap();
    file_a.write(b"second").await.unwrap();
    file_a.flush().await.unwrap();

    // Re-register B's repo
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    // Wait until the file is updated
    expect_file_content(&repo_b, "test.txt", b"second").await;
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

    expect_file_content(&repo_b, "test.txt", &[]).await;

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
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    write_in_chunks(&mut file, &content, 4096).await;
    file.flush().await.unwrap();
    drop(file);

    tracing::info!("reading");
    expect_file_content(&repo_b, "test.dat", &content).await;
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
    write_in_chunks(&mut file, &content, 4096).await;
    file.flush().await.unwrap();
    drop(file);

    expect_file_content(&repo_b, "test.dat", &content).await;
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
        write_in_chunks(&mut file, content, 4096).await;
        file.flush().await.unwrap();
        drop(file);

        // Wait until we see all the already transfered files
        for (index, content) in contents.iter().take(index + 1).enumerate() {
            let name = format!("file-{}.dat", index);
            expect_file_content(&repo_b, &name, content).await;
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

    let mut content = vec![0; 3 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content[..]);

    // A: Create empty file
    let mut file_a = repo_a.create_file("foo.txt").await.unwrap();

    // B: Wait until everything gets merged
    expect_in_sync(&repo_b, &repo_a).await;

    // A: Write half of the file content but don't flush yet.
    write_in_chunks(&mut file_a, &content[..content.len() / 2], 4096).await;

    // B: Write a file. Excluding the unflushed changes by A, this makes B's branch newer than
    // A's.
    let mut file_b = repo_b.create_file("bar.txt").await.unwrap();
    file_b.write(b"bar").await.unwrap();
    file_b.flush().await.unwrap();
    drop(file_b);

    // A: Wait until we see the file created by B
    expect_file_content(&repo_a, "bar.txt", b"bar").await;

    // A: Write the second half of the content and flush.
    write_in_chunks(&mut file_a, &content[content.len() / 2..], 4096).await;
    file_a.flush().await.unwrap();

    // A: Reopen the file and verify it has the expected full content
    let mut file_a = repo_a.open_file("foo.txt").await.unwrap();
    let actual_content = file_a.read_to_end().await.unwrap();
    assert_eq!(actual_content, content);

    // B: Wait until we see the file as well
    expect_file_content(&repo_b, "foo.txt", &content).await;
}

// Test that merge is not affected by files from remote branches being open while it's ongoing.
#[tokio::test(flavor = "multi_thread")]
async fn sync_during_file_read() {
    let mut env = Env::with_seed(0);

    let mut content_a = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content_a[..]);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;

    loop {
        let (repo_a, repo_b) = env.create_linked_repos().await;

        // A: create an initially empty file
        let mut file_a = repo_a.create_file("foo.txt").await.unwrap();
        let branch_id_a = *file_a.branch().id();

        // B: Establish the link and wait until the file gets synced but not merged.
        let reg_a = node_a.network.handle().register(repo_a.store().clone());
        let reg_b = node_b.network.handle().register(repo_b.store().clone());

        let mut file_b = None;
        let mut tx = repo_b.subscribe();

        loop {
            match repo_b.open_file("foo.txt").await {
                Ok(file) => {
                    // Only use the file if it's still in the remote branch, otherwise the test
                    // preconditions are not met and we need to restart the test.
                    if file.branch().id() == &branch_id_a {
                        file_b = Some(file);
                    }

                    break;
                }
                Err(Error::EntryNotFound | Error::BlockNotFound(_)) => common::wait(&mut tx).await,
                Err(error) => panic!("unexpected error: {:?}", error),
            }
        }

        let file_b = if let Some(file) = file_b {
            file
        } else {
            // File was already forked which breaks the preconditions of this test. Need to try
            // again.
            // TODO: a better way would be to initially somehow pause B's merger and resume it
            // after the file is opened.

            tracing::warn!("precondition failed, trying again");

            drop(reg_a);
            drop(reg_b);

            repo_a.close().await.unwrap();
            repo_b.close().await.unwrap();

            continue;
        };

        // B: Keep the file open

        // A: write at least two blocks worth of data to the file
        write_in_chunks(&mut file_a, &content_a, 4096).await;
        file_a.flush().await.unwrap();

        // B: Wait until we are synced with A, still keeping the file open
        expect_in_sync(&repo_b, &repo_a).await;

        // B: Drop and reopen the file and verify that the fact we held the file open did not
        // interfere with the merging and that the file is now identical to A's file.
        drop(file_b);

        let branch_id_b = *repo_b.local_branch().unwrap().id();
        expect_file_version_content(&repo_b, "foo.txt", Some(&branch_id_b), &content_a).await;

        break;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_modify_open_file() {
    let mut env = Env::with_seed(0);

    let (node_a, node_b) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    let mut content_a = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    let mut content_b = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content_a[..]);
    env.rng.fill(&mut content_b[..]);

    // A: Create empty file
    let mut file_a = repo_a.create_file("file.txt").await.unwrap();

    // B: Wait until everything gets merged
    expect_in_sync(&repo_b, &repo_a).await;

    // A: Write to file but don't flush yet
    write_in_chunks(&mut file_a, &content_a, 4096).await;

    // B: Write to the same file and flush
    let mut file_b = repo_b.open_file("file.txt").await.unwrap();
    write_in_chunks(&mut file_b, &content_b, 4096).await;
    file_b.flush().await.unwrap();

    drop(file_b);

    let id_a = *repo_a.local_branch().unwrap().id();
    let id_b = *repo_b.local_branch().unwrap().id();

    // A: Wait until we see B's writes
    expect_file_version_content(&repo_a, "file.txt", Some(&id_b), &content_b).await;

    // A: Flush the file
    file_a.flush().await.unwrap();
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
    drop(repo_a);
    let repo_a = Repository::open_with_mode(&store_a, device_id_a, None, AccessMode::Read)
        .await
        .unwrap();

    // A + B: establish link
    let _reg_a = node_a.network.handle().register(repo_a.store().clone());
    let _reg_b = node_b.network.handle().register(repo_b.store().clone());

    // B: Sync with A
    expect_in_sync(&repo_b, &repo_a).await;

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
    expect_file_content(&repo_a, "foo.txt", b"hello from A\nhello from B\n").await;

    // A: Reopen in write mode
    drop(repo_a);
    let repo_a = Repository::open(&store_a, device_id_a, None).await.unwrap();

    // A: Modify the repo
    repo_a.create_file("bar.txt").await.unwrap();

    // A: Make sure the local version changed monotonically.
    common::eventually(&repo_a, || async {
        let vv_a_1 = repo_a
            .local_branch()
            .unwrap()
            .version_vector()
            .await
            .unwrap();

        match vv_a_1.partial_cmp(&vv_b) {
            Some(Ordering::Greater) => true,
            Some(Ordering::Equal | Ordering::Less) => {
                panic!("non-monotonic version progression")
            }
            None => false,
        }
    })
    .await;
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

                write_in_chunks(&mut file, content, 4096).await;
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

                    if !check_file_version_content(&repo_b, &name, None, content).await {
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

    expect_file_version_content(&repo_a, "data.txt", Some(&id_a), &content).await;

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
    expect_file_version_content(&repo_a, "data.txt", Some(&id_b), &content).await;
}

// Wait until the file at `path` has the expected content. Panics if timeout elapses before the
// file content matches.
async fn expect_file_content(repo: &Repository, path: &str, expected_content: &[u8]) {
    expect_file_version_content(repo, path, None, expected_content).await
}

async fn expect_file_version_content(
    repo: &Repository,
    path: &str,
    branch_id: Option<&PublicKey>,
    expected_content: &[u8],
) {
    common::eventually(repo, || {
        check_file_version_content(repo, path, branch_id, expected_content)
            .instrument(Span::current())
    })
    .await
}

async fn check_file_version_content(
    repo: &Repository,
    path: &str,
    branch_id: Option<&PublicKey>,
    expected_content: &[u8],
) -> bool {
    tracing::debug!(path, "opening");

    let result = if let Some(branch_id) = branch_id {
        repo.open_file_version(path, branch_id).await
    } else {
        repo.open_file(path).await
    };

    let mut file = match result {
        Ok(file) => file,
        // `EntryNotFound` likely means that the parent directory hasn't yet been fully synced
        // and so the file entry is not in it yet.
        //
        // `BlockNotFound` means the first block of the file hasn't been downloaded yet.
        Err(error @ (Error::EntryNotFound | Error::BlockNotFound(_))) => {
            tracing::warn!(path, ?error, "open failed");
            return false;
        }
        Err(error) => panic!("unexpected error: {:?}", error),
    };

    tracing::debug!(path, "opened");

    let actual_content = match read_in_chunks(&mut file, 4096).await {
        Ok(content) => content,
        // `EntryNotFound` can still happen even here if merge runs in the middle of reading
        // the file - we opened the file while it was still in the remote branch but then that
        // branch got merged into the local one and deleted. That means the file no longer
        // exists in the remote branch and attempt to read from it further results in this
        // error.
        // TODO: this is not ideal as the only way to resolve this problem is to reopen the
        // file (unlike the `BlockNotFound` error where we just need to read it again when the
        // block gets downloaded). This should probably be considered a bug.
        //
        // `BlockNotFound` means just the some block of the file hasn't been downloaded yet.
        Err(error @ (Error::EntryNotFound | Error::BlockNotFound(_))) => {
            tracing::warn!(path, ?error, "read failed");
            return false;
        }
        Err(error) => panic!("unexpected error: {:?}", error),
    };

    if actual_content == expected_content {
        tracing::debug!(path, "content matches");
        true
    } else {
        tracing::warn!(path, "content does not match");
        false
    }
}

// Wait until A is in sync with B, that is: both repos have local branches, they have non-empty
// version vectors and A's version vector is greater or equal to B's.
async fn expect_in_sync(repo_a: &Repository, repo_b: &Repository) {
    common::eventually(repo_a, || async {
        let vv_a = repo_a
            .local_branch()
            .unwrap()
            .version_vector()
            .await
            .unwrap();

        let vv_b = repo_b
            .local_branch()
            .unwrap()
            .version_vector()
            .await
            .unwrap();

        if vv_a.is_empty() || vv_b.is_empty() {
            return false;
        }

        vv_a >= vv_b
    })
    .await
}

async fn expect_entry_exists(repo: &Repository, path: &str, entry_type: EntryType) {
    common::eventually(repo, || async {
        let result = match entry_type {
            EntryType::File => repo.open_file(path).await.map(|_| ()),
            EntryType::Directory => repo.open_directory(path).await.map(|_| ()),
        };

        match result {
            Ok(()) => true,
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => false,
            Err(error) => panic!("unexpected error: {:?}", error),
        }
    })
    .await
}

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

async fn write_in_chunks(file: &mut File, content: &[u8], chunk_size: usize) {
    for offset in (0..content.len()).step_by(chunk_size) {
        let end = (offset + chunk_size).min(content.len());
        file.write(&content[offset..end]).await.unwrap();

        if to_megabytes(end) > to_megabytes(offset) {
            tracing::debug!(
                "file write progress: {}/{} MB",
                to_megabytes(end),
                to_megabytes(content.len())
            );
        }
    }
}

async fn read_in_chunks(file: &mut File, chunk_size: usize) -> Result<Vec<u8>, Error> {
    let mut content = vec![0; file.len() as usize];
    let mut offset = 0;

    while offset < content.len() {
        let end = (offset + chunk_size).min(content.len());
        let size = file.read(&mut content[offset..end]).await?;
        offset += size;
    }

    Ok(content)
}

fn to_megabytes(bytes: usize) -> usize {
    bytes / 1024 / 1024
}

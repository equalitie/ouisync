mod common;

use self::common::{Env, Proto, DEFAULT_TIMEOUT};
use assert_matches::assert_matches;
use camino::Utf8Path;
use ouisync::{
    crypto::sign::PublicKey, db, network::Network, AccessMode, AccessSecrets, ConfigStore,
    EntryType, Error, File, MasterSecret, Repository, StateMonitor, BLOB_HEADER_SIZE, BLOCK_SIZE,
};
use rand::Rng;
use std::{cmp::Ordering, io::SeekFrom, net::Ipv4Addr, sync::Arc, time::Duration};
use tokio::{select, task, time};
use tracing::{Instrument, Span};

#[tokio::test(flavor = "multi_thread")]
async fn relink_repository() {
    let mut env = Env::with_seed(0);

    // Create two peers and connect them together.
    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;

    let (repo_a, repo_b) = env.create_linked_repos().await;

    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let reg_b = network_b.handle().register(repo_b.store().clone());

    // Create a file by A
    let mut file_a = repo_a.create_file("test.txt").await.unwrap();
    let mut conn = repo_a.db().acquire().await.unwrap();
    file_a.write(&mut conn, b"first").await.unwrap();
    file_a.flush(&mut conn).await.unwrap();

    // Wait until the file is seen by B
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", b"first"),
    )
    .await
    .unwrap();

    // Unlink B's repo
    drop(reg_b);

    // Update the file while B's repo is unlinked
    let mut conn = repo_a.db().acquire().await.unwrap();
    file_a.truncate(&mut conn, 0).await.unwrap();
    file_a.write(&mut conn, b"second").await.unwrap();
    file_a.flush(&mut conn).await.unwrap();

    // Re-register B's repo
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    // Wait until the file is updated
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", b"second"),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_remote_file() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    // Create a file by A and wait until B sees it.
    let mut file = repo_a.create_file("test.txt").await.unwrap();
    let mut conn = repo_a.db().acquire().await.unwrap();
    file.flush(&mut conn).await.unwrap();
    drop(conn);

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", &[]),
    )
    .await
    .unwrap();

    // Delete the file by B
    repo_b.remove_entry("test.txt").await.unwrap();

    // TODO: wait until A sees the file being deleted
}

#[tokio::test(flavor = "multi_thread")]
async fn relay() {
    // Simulate two peers that can't connect to each other but both can connect to a third peer.

    // There used to be a deadlock that got triggered only when transferring a sufficiently large
    // file.
    let file_size = 4 * 1024 * 1024;

    let mut env = Env::with_seed(0);
    let proto = Proto::Tcp;

    // The "relay" peer.
    let network_r = Network::new(
        &common::test_bind_addrs(proto),
        ConfigStore::null(),
        StateMonitor::make_root(),
    );
    network_r.handle().enable().await;

    let network_a =
        common::create_peer_connected_to(proto.listener_local_addr_v4(&network_r)).await;
    let network_b =
        common::create_peer_connected_to(proto.listener_local_addr_v4(&network_r)).await;

    let repo_a = env.create_repo().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());

    let repo_b = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let repo_r = env
        .create_repo_with_secrets(repo_a.secrets().with_mode(AccessMode::Blind))
        .await;
    let _reg_r = network_r.handle().register(repo_r.store().clone());

    let mut content = vec![0; file_size];
    env.rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it. The file must pass through R because A and B
    // are not connected to each other.
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    let mut conn = repo_a.db().acquire().await.unwrap();
    write_in_chunks(&mut conn, &mut file, &content, 4096).await;
    file.flush(&mut conn).await.unwrap();
    drop(file);

    time::timeout(
        Duration::from_secs(60),
        expect_file_content(&repo_b, "test.dat", &content),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_large_file() {
    let file_size = 4 * 1024 * 1024;

    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let mut content = vec![0; file_size];
    env.rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it.
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    let mut conn = repo_a.db().acquire().await.unwrap();
    write_in_chunks(&mut conn, &mut file, &content, 4096).await;
    file.flush(&mut conn).await.unwrap();
    drop(file);

    time::timeout(
        Duration::from_secs(60),
        expect_file_content(&repo_b, "test.dat", &content),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_multiple_files_sequentially() {
    let mut env = Env::with_seed(0);
    let file_sizes = [512 * 1024, 1024];

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

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
        let mut conn = repo_a.db().acquire().await.unwrap();
        write_in_chunks(&mut conn, &mut file, content, 4096).await;
        file.flush(&mut conn).await.unwrap();
        drop(file);

        // Wait until we see all the already transfered files
        for (index, content) in contents.iter().take(index + 1).enumerate() {
            let name = format!("file-{}.dat", index);
            time::timeout(
                DEFAULT_TIMEOUT,
                expect_file_content(&repo_b, &name, content),
            )
            .await
            .unwrap();
        }
    }
}

// Test for an edge case where a sync happens while we are in the middle of writing a file.
// This test makes sure that when the sync happens, the partially written file content is not
// garbage collected prematurelly.
#[tokio::test(flavor = "multi_thread")]
async fn sync_during_file_write() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let mut content = vec![0; 3 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content[..]);

    // A: Create empty file
    let mut file_a = repo_a.create_file("foo.txt").await.unwrap();

    // B: Wait until everything gets merged
    time::timeout(DEFAULT_TIMEOUT, expect_in_sync(&repo_b, &repo_a))
        .await
        .unwrap();

    // A: Write half of the file content but don't flush yet.
    write_in_chunks(
        &mut repo_a.db().acquire().await.unwrap(),
        &mut file_a,
        &content[..content.len() / 2],
        4096,
    )
    .await;

    // B: Write a file. Excluding the unflushed changes by A, this makes B's branch newer than
    // A's.
    let mut file_b = repo_b.create_file("bar.txt").await.unwrap();
    let mut conn = repo_b.db().acquire().await.unwrap();
    file_b.write(&mut conn, b"bar").await.unwrap();
    file_b.flush(&mut conn).await.unwrap();
    drop(file_b);

    // A: Wait until we see the file created by B
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_a, "bar.txt", b"bar"),
    )
    .await
    .unwrap();

    // A: Write the second half of the content and flush.
    let mut conn = repo_a.db().acquire().await.unwrap();
    write_in_chunks(&mut conn, &mut file_a, &content[content.len() / 2..], 4096).await;
    file_a.flush(&mut conn).await.unwrap();

    // A: Reopen the file and verify it has the expected full content
    let mut file_a = repo_a.open_file("foo.txt").await.unwrap();
    let actual_content = file_a.read_to_end(&mut conn).await.unwrap();
    assert_eq!(actual_content, content);

    // B: Wait until we see the file as well
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "foo.txt", &content),
    )
    .await
    .unwrap();
}

// Test that merge is not affected by files from remote branches being open while it's ongoing.
#[tokio::test(flavor = "multi_thread")]
async fn sync_during_file_read() {
    let mut env = Env::with_seed(0);

    let mut content_a = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content_a[..]);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;

    loop {
        let (repo_a, repo_b) = env.create_linked_repos().await;

        // A: create an initially empty file
        let mut file_a = repo_a.create_file("foo.txt").await.unwrap();
        let branch_id_a = *file_a.branch().id();

        // B: Establish the link and wait until the file gets synced but not merged.
        let reg_a = network_a.handle().register(repo_a.store().clone());
        let reg_b = network_b.handle().register(repo_b.store().clone());

        let mut file_b = None;
        let mut tx = repo_b.subscribe();

        time::timeout(DEFAULT_TIMEOUT, async {
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
                    Err(Error::EntryNotFound | Error::BlockNotFound(_)) => {
                        common::wait(&mut tx).await
                    }
                    Err(error) => panic!("unexpected error: {:?}", error),
                }
            }
        })
        .await
        .unwrap();

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

            repo_a.close().await;
            repo_b.close().await;

            continue;
        };

        // B: Keep the file open

        // A: write at least two blocks worth of data to the file
        let mut conn = repo_a.db().acquire().await.unwrap();
        write_in_chunks(&mut conn, &mut file_a, &content_a, 4096).await;
        file_a.flush(&mut conn).await.unwrap();

        // B: Wait until we are synced with A, still keeping the file open
        time::timeout(DEFAULT_TIMEOUT, expect_in_sync(&repo_b, &repo_a))
            .await
            .unwrap();

        // B: Drop and reopen the file and verify that the fact we held the file open did not
        // interfere with the merging and that the file is now identical to A's file.
        drop(file_b);

        let branch_id_b = *repo_b.local_branch().unwrap().id();
        time::timeout(
            DEFAULT_TIMEOUT,
            expect_file_version_content(&repo_b, "foo.txt", Some(&branch_id_b), &content_a),
        )
        .await
        .unwrap();

        break;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_modify_open_file() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let mut content_a = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    let mut content_b = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    env.rng.fill(&mut content_a[..]);
    env.rng.fill(&mut content_b[..]);

    // A: Create empty file
    let mut file_a = repo_a.create_file("file.txt").await.unwrap();

    // B: Wait until everything gets merged
    time::timeout(DEFAULT_TIMEOUT, expect_in_sync(&repo_b, &repo_a))
        .await
        .unwrap();

    // A: Write to file but don't flush yet
    write_in_chunks(
        &mut repo_a.db().acquire().await.unwrap(),
        &mut file_a,
        &content_a,
        4096,
    )
    .await;

    // B: Write to the same file and flush
    let mut file_b = repo_b.open_file("file.txt").await.unwrap();
    let mut conn = repo_b.db().acquire().await.unwrap();
    write_in_chunks(&mut conn, &mut file_b, &content_b, 4096).await;
    file_b.flush(&mut conn).await.unwrap();
    drop(conn);

    drop(file_b);

    let id_a = *repo_a.local_branch().unwrap().id();
    let id_b = *repo_b.local_branch().unwrap().id();

    // A: Wait until we see B's writes
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_version_content(&repo_a, "file.txt", Some(&id_b), &content_b),
    )
    .await
    .unwrap();

    // A: Flush the file
    let mut conn = repo_a.db().acquire().await.unwrap();
    file_a.flush(&mut conn).await.unwrap();
    drop(conn);
    drop(file_a);

    // A: Verify both versions of the file are still present
    assert_matches!(
        repo_a.open_file("file.txt").await,
        Err(Error::AmbiguousEntry)
    );

    let mut file_a = repo_a.open_file_version("file.txt", &id_a).await.unwrap();
    let mut conn = repo_a.db().acquire().await.unwrap();
    let actual_content_a = file_a.read_to_end(&mut conn).await.unwrap();
    drop(conn);

    let mut file_b = repo_a.open_file_version("file.txt", &id_b).await.unwrap();
    let mut conn = repo_a.db().acquire().await.unwrap();
    let actual_content_b = file_b.read_to_end(&mut conn).await.unwrap();
    drop(conn);

    assert!(actual_content_a == content_a);
    assert!(actual_content_b == content_b);
}

// Test that the local version changes monotonically even when the local branch temporarily becomes
// outdated.
#[tokio::test(flavor = "multi_thread")]
async fn recreate_local_branch() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;

    let store_a = env.next_store();
    let device_id_a = env.rng.gen();
    let master_secret_a = MasterSecret::generate(&mut env.rng);
    let access_secrets = AccessSecrets::generate_write(&mut env.rng);
    let repo_a = Repository::create(
        &store_a,
        device_id_a,
        master_secret_a.clone(),
        access_secrets.clone(),
        true,
        &StateMonitor::make_root(),
    )
    .await
    .unwrap();

    let repo_b = env.create_repo_with_secrets(access_secrets.clone()).await;

    let mut file = repo_a.create_file("foo.txt").await.unwrap();
    let mut conn = repo_a.db().acquire().await.unwrap();
    file.write(&mut conn, b"hello from A\n").await.unwrap();
    file.flush(&mut conn).await.unwrap();

    let vv_a_0 = repo_a
        .local_branch()
        .unwrap()
        .version_vector(&mut conn)
        .await
        .unwrap();

    drop(conn);
    drop(file);

    // A: Reopen the repo in read mode to disable merger
    drop(repo_a);
    let repo_a = Repository::open_with_mode(
        &store_a,
        device_id_a,
        Some(master_secret_a.clone()),
        AccessMode::Read,
        true,
        &StateMonitor::make_root(),
    )
    .await
    .unwrap();

    // A + B: establish link
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    // B: Sync with A
    time::timeout(DEFAULT_TIMEOUT, expect_in_sync(&repo_b, &repo_a))
        .await
        .unwrap();

    // B: Modify the repo. This makes B's branch newer than A's
    let mut file = repo_b.open_file("foo.txt").await.unwrap();
    let mut conn = repo_b.db().acquire().await.unwrap();
    file.seek(&mut conn, SeekFrom::End(0)).await.unwrap();
    file.write(&mut conn, b"hello from B\n").await.unwrap();
    file.flush(&mut conn).await.unwrap();

    let vv_b = repo_b
        .local_branch()
        .unwrap()
        .version_vector(&mut conn)
        .await
        .unwrap();

    drop(conn);
    drop(file);

    assert!(vv_b > vv_a_0);

    // A: Sync with B. Afterwards our local branch will become outdated compared to B's
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_a, "foo.txt", b"hello from A\nhello from B\n"),
    )
    .await
    .unwrap();

    // A: Reopen in write mode
    drop(repo_a);
    let repo_a = Repository::open(
        &store_a,
        device_id_a,
        Some(master_secret_a),
        true,
        &StateMonitor::make_root(),
    )
    .await
    .unwrap();

    // A: Modify the repo
    repo_a.create_file("bar.txt").await.unwrap();

    // A: Make sure the local version changed monotonically.
    time::timeout(
        DEFAULT_TIMEOUT,
        common::eventually(&repo_a, || async {
            let mut conn = repo_a.db().acquire().await.unwrap();
            let vv_a_1 = repo_a
                .local_branch()
                .unwrap()
                .version_vector(&mut conn)
                .await
                .unwrap();

            match vv_a_1.partial_cmp(&vv_b) {
                Some(Ordering::Greater) => true,
                Some(Ordering::Equal | Ordering::Less) => {
                    panic!("non-monotonic version progression")
                }
                None => false,
            }
        }),
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_many_files() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Quic).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

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
                let mut file = repo_a
                    .create_file(format!("file-{}.dat", index))
                    .await
                    .unwrap();

                let mut conn = repo_a.db().acquire().await.unwrap();
                write_in_chunks(&mut conn, &mut file, content, 4096).await;
                file.flush(&mut conn).await.unwrap();

                // println!("put {}/{}", index + 1, contents.len());
            }
        }
        .instrument(Span::current())
    });

    let task_b = task::spawn(
        async move {
            common::eventually(&repo_b, || async {
                for (index, content) in contents.iter().enumerate() {
                    if !check_file_version_content(
                        &repo_b,
                        &format!("file-{}.dat", index),
                        None,
                        content,
                    )
                    .await
                    {
                        return false;
                    }

                    // println!("get {}/{}", index + 1, contents.len());
                }

                true
            })
            .await
        }
        .instrument(Span::current()),
    );

    time::timeout(Duration::from_secs(30), async move {
        task_a.await.unwrap();
        task_b.await.unwrap();
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_directory_with_file() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let mut conn = repo_a.db().acquire().await.unwrap();

    let mut dir = repo_a.create_directory("food").await.unwrap();
    dir.create_file(&mut conn, "pizza.jpg".into())
        .await
        .unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_b, "food/pizza.jpg", EntryType::File),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_directory_with_subdirectory() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let mut conn = repo_a.db().acquire().await.unwrap();

    let mut dir0 = repo_a.create_directory("food").await.unwrap();
    dir0.create_directory(&mut conn, "mediterranean".into())
        .await
        .unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_b, "food/mediterranean", EntryType::Directory),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_file() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    repo_b.create_file("foo.txt").await.unwrap();

    // Wait until the file is synced and merged over to the local branch.
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "foo.txt", EntryType::File),
    )
    .await
    .unwrap();

    repo_b
        .move_entry("/", "foo.txt", "/", "bar.txt")
        .await
        .unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "bar.txt", EntryType::File),
    )
    .await
    .unwrap();

    time::timeout(DEFAULT_TIMEOUT, expect_entry_not_found(&repo_a, "foo.txt"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_empty_directory() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    repo_b.create_directory("foo").await.unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "foo", EntryType::Directory),
    )
    .await
    .unwrap();

    repo_b.move_entry("/", "foo", "/", "bar").await.unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "bar", EntryType::Directory),
    )
    .await
    .unwrap();

    time::timeout(DEFAULT_TIMEOUT, expect_entry_not_found(&repo_a, "foo"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_non_empty_directory() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let mut dir = repo_b.create_directory("foo").await.unwrap();
    let mut conn = repo_b.db().acquire().await.unwrap();
    dir.create_file(&mut conn, "data.txt".into()).await.unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "foo/data.txt", EntryType::File),
    )
    .await
    .unwrap();

    repo_b.move_entry("/", "foo", "/", "bar").await.unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "bar/data.txt", EntryType::File),
    )
    .await
    .unwrap();

    time::timeout(DEFAULT_TIMEOUT, expect_entry_not_found(&repo_a, "foo"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_move_file_to_directory_then_rename_that_directory() {
    let mut env = Env::with_seed(0);

    let (network_a, network_b) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_a, repo_b) = env.create_linked_repos().await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    repo_b.create_file("data.txt").await.unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "data.txt", EntryType::File),
    )
    .await
    .unwrap();

    repo_b.create_directory("archive").await.unwrap();
    repo_b
        .move_entry("/", "data.txt", "archive", "data.txt")
        .await
        .unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "archive/data.txt", EntryType::File),
    )
    .await
    .unwrap();

    repo_b
        .move_entry("/", "archive", "/", "trash")
        .await
        .unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_entry_exists(&repo_a, "trash/data.txt", EntryType::File),
    )
    .await
    .unwrap();

    time::timeout(DEFAULT_TIMEOUT, expect_entry_not_found(&repo_a, "data.txt"))
        .await
        .unwrap();

    time::timeout(DEFAULT_TIMEOUT, expect_entry_not_found(&repo_a, "archive"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_exchange() {
    let mut env = Env::with_seed(0);
    let proto = Proto::Quic;

    // B and C are initially connected only to A...
    let network_a = common::create_disconnected_peer(proto).await;
    let network_b =
        common::create_peer_connected_to(proto.listener_local_addr_v4(&network_a)).await;
    let network_c =
        common::create_peer_connected_to(proto.listener_local_addr_v4(&network_a)).await;

    let repo_a = env.create_repo().await;
    let reg_a = network_a.handle().register(repo_a.store().clone());
    reg_a.enable_pex();

    let repo_b = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let reg_b = network_b.handle().register(repo_b.store().clone());
    reg_b.enable_pex();

    let repo_c = env.create_repo_with_secrets(repo_a.secrets().clone()).await;
    let reg_c = network_c.handle().register(repo_c.store().clone());
    reg_c.enable_pex();

    let addr_b = proto.listener_local_addr_v4(&network_b);
    let addr_c = proto.listener_local_addr_v4(&network_c);

    let mut rx_b = network_b.on_peer_set_change();
    let mut rx_c = network_c.on_peer_set_change();

    // ...eventually B and C connect to each other via peer exchange.
    let connected = async {
        loop {
            if network_b.knows_peer(addr_c) && network_c.knows_peer(addr_b) {
                break;
            }

            select! {
                result = rx_b.changed() => result.unwrap(),
                result = rx_c.changed() => result.unwrap(),
            }
        }
    };

    time::timeout(DEFAULT_TIMEOUT, connected).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_idle() {
    let _env = Env::with_seed(0);
    let proto = Proto::Quic;

    let network = common::create_disconnected_peer(proto).await;
    let local_addr_0 = proto.listener_local_addr_v4(&network);

    network.handle().disable();
    network.handle().enable().await;

    let local_addr_1 = proto.listener_local_addr_v4(&network);
    assert_eq!(local_addr_1, local_addr_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_pending_connection() {
    let _env = Env::with_seed(0);
    let proto = Proto::Quic;

    let network = common::create_disconnected_peer(proto).await;
    let local_addr_0 = proto.listener_local_addr_v4(&network);

    let remote_addr = proto.wrap_addr((Ipv4Addr::LOCALHOST, 12345).into());
    network.add_user_provided_peer(&remote_addr);

    // Wait until the connection starts begin established.
    let mut rx = network.on_peer_set_change();
    time::timeout(DEFAULT_TIMEOUT, async {
        loop {
            if network.knows_peer(remote_addr) {
                break;
            }

            rx.changed().await.unwrap();
        }
    })
    .await
    .unwrap();

    network.handle().disable();
    network.handle().enable().await;

    let local_addr_1 = proto.listener_local_addr_v4(&network);
    assert_eq!(local_addr_1, local_addr_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn network_disable_enable_addr_takeover() {
    use tokio::net::UdpSocket;

    let _env = Env::with_seed(0);
    let proto = Proto::Quic;

    let network = common::create_disconnected_peer(proto).await;
    let local_addr_0 = proto.listener_local_addr_v4(&network);

    network.handle().disable();

    // Bind some other socket to the same address while the network is disabled.
    let _socket = time::timeout(DEFAULT_TIMEOUT, async {
        loop {
            if let Ok(socket) = UdpSocket::bind(local_addr_0.socket_addr()).await {
                break socket;
            } else {
                time::sleep(Duration::from_millis(250)).await;
            }
        }
    })
    .await
    .unwrap();

    // Enabling the network binds it to a different address.
    network.handle().enable().await;

    let local_addr_1 = proto.listener_local_addr_v4(&network);
    assert_ne!(local_addr_1, local_addr_0);
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
            tracing::debug!(path, ?error, "open failed");
            return false;
        }
        Err(error) => panic!("unexpected error: {:?}", error),
    };

    tracing::debug!(path, "opened");

    let actual_content = match read_in_chunks(repo.db(), &mut file, 4096).await {
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
            tracing::debug!(path, ?error, "read failed");
            return false;
        }
        Err(error) => panic!("unexpected error: {:?}", error),
    };

    if actual_content == expected_content {
        tracing::debug!(path, "content matches");
        true
    } else {
        tracing::debug!(path, "content does not match");
        false
    }
}

// Wait until A is in sync with B, that is: both repos have local branches, they have non-empty
// version vectors and A's version vector is greater or equal to B's.
async fn expect_in_sync(repo_a: &Repository, repo_b: &Repository) {
    common::eventually(repo_a, || async {
        let vv_a = {
            let branch = repo_a.local_branch().unwrap();
            let mut conn = repo_a.db().acquire().await.unwrap();
            branch.version_vector(&mut conn).await.unwrap()
        };

        let vv_b = {
            let branch = repo_b.local_branch().unwrap();
            let mut conn = repo_b.db().acquire().await.unwrap();
            branch.version_vector(&mut conn).await.unwrap()
        };

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

async fn write_in_chunks(
    conn: &mut db::Connection,
    file: &mut File,
    content: &[u8],
    chunk_size: usize,
) {
    for offset in (0..content.len()).step_by(chunk_size) {
        let end = (offset + chunk_size).min(content.len());
        file.write(conn, &content[offset..end]).await.unwrap();

        if to_megabytes(end) > to_megabytes(offset) {
            tracing::debug!(
                "file write progress: {}/{} MB",
                to_megabytes(end),
                to_megabytes(content.len())
            );
        }
    }
}

async fn read_in_chunks(
    db: &db::Pool,
    file: &mut File,
    chunk_size: usize,
) -> Result<Vec<u8>, Error> {
    let mut content = vec![0; file.len().await as usize];
    let mut offset = 0;

    while offset < content.len() {
        let mut conn = db.acquire().await?;

        let end = (offset + chunk_size).min(content.len());
        let size = file.read(&mut conn, &mut content[offset..end]).await?;
        offset += size;
    }

    Ok(content)
}

fn to_megabytes(bytes: usize) -> usize {
    bytes / 1024 / 1024
}

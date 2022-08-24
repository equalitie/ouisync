mod common;

use self::common::{Env, Proto};
use assert_matches::assert_matches;
use camino::Utf8Path;
use ouisync::{
    crypto::sign::PublicKey, db, network::Network, AccessMode, AccessSecrets, ConfigStore,
    EntryType, Error, File, MasterSecret, Repository, StateMonitor, BLOB_HEADER_SIZE, BLOCK_SIZE,
};
use rand::Rng;
use std::{cmp::Ordering, io::SeekFrom, sync::Arc, time::Duration};
use tokio::{task, time};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

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
        &common::test_network_options(proto),
        ConfigStore::null(),
        StateMonitor::make_root(),
    )
    .await
    .unwrap();

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
    let file_sizes = [512 * 1024, 1024];

    let mut env = Env::with_seed(0);

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
    // common::init_log();

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
    });

    let task_b = task::spawn(async move {
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
    });

    time::timeout(Duration::from_secs(30), async move {
        task_a.await.unwrap();
        task_b.await.unwrap();
    })
    .await
    .unwrap()
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

// FIXME: this fails because forked directory version vector are not correctly assigned.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn remote_rename_directory() {
    // common::init_log();

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
    })
    .await
}

async fn check_file_version_content(
    repo: &Repository,
    path: &str,
    branch_id: Option<&PublicKey>,
    expected_content: &[u8],
) -> bool {
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
        Err(Error::EntryNotFound | Error::BlockNotFound(_)) => return false,
        Err(error) => panic!("unexpected error: {:?}", error),
    };

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
        Err(Error::EntryNotFound | Error::BlockNotFound(_)) => return false,
        Err(error) => panic!("unexpected error: {:?}", error),
    };

    actual_content == expected_content
}

// Wait until A is in sync with B, that is: both repos have local branches, they have non-empty
// version vectors and A's version vector is greater or equal to B's.
async fn expect_in_sync(repo_a: &Repository, repo_b: &Repository) {
    common::eventually(repo_a, || async {
        let vv_a = if let Some(branch) = repo_a.local_branch() {
            let mut conn = repo_a.db().acquire().await.unwrap();
            branch.version_vector(&mut conn).await.unwrap()
        } else {
            return false;
        };

        let vv_b = if let Some(branch) = repo_b.local_branch() {
            let mut conn = repo_b.db().acquire().await.unwrap();
            branch.version_vector(&mut conn).await.unwrap()
        } else {
            return false;
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

    let parent = repo.open_directory(parent).await.unwrap();

    common::eventually(repo, || async {
        matches!(parent.lookup_unique(name), Err(Error::EntryNotFound))
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

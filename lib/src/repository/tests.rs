use super::*;
use crate::{
    blob, db,
    protocol::{BlockId, BLOCK_NONCE_SIZE, BLOCK_SIZE},
    test_utils, WriteSecrets,
};
use assert_matches::assert_matches;
use rand::Rng;
use std::{future::Future, io::SeekFrom};
use tempfile::TempDir;
use tokio::{
    sync::broadcast::Receiver,
    time::{self, timeout, Duration},
};
use tracing::instrument;

#[tokio::test(flavor = "multi_thread")]
async fn root_directory_always_exists() {
    let (_base_dir, repo) = setup().await;
    let _ = repo.open_directory("/").await.unwrap();
}

// Count leaf nodes in the index of the local branch.
async fn count_local_index_leaf_nodes(repo: &Repository) -> usize {
    let branch = repo.local_branch().unwrap();
    repo.shared
        .vault
        .store()
        .acquire_read()
        .await
        .unwrap()
        .count_leaf_nodes_in_branch(branch.id())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn count_leaf_nodes_sanity_checks() {
    let (_base_dir, repo) = setup().await;

    let file_name = "test.txt";

    //------------------------------------------------------------------------
    // Create a small file in the root.

    let mut file = repo.create_file(file_name).await.unwrap();
    file.write_all(&random_bytes(BLOCK_SIZE - blob::HEADER_SIZE))
        .await
        .unwrap();
    file.flush().await.unwrap();

    // 2 = one for the root + one for the file.
    assert_eq!(count_local_index_leaf_nodes(&repo).await, 2);

    //------------------------------------------------------------------------
    // Make the file bigger to expand to two blocks

    file.write_all(&random_bytes(1)).await.unwrap();
    file.flush().await.unwrap();

    // 3 = one for the root + two for the file.
    assert_eq!(count_local_index_leaf_nodes(&repo).await, 3);

    //------------------------------------------------------------------------
    // Remove the file, we should end up with just one block for the root.

    drop(file);
    repo.remove_entry(file_name).await.unwrap();

    //------------------------------------------------------------------------
    // 1 = one for the root with a tombstone entry
    wait_for(&repo, || async {
        let actual = count_local_index_leaf_nodes(&repo).await;
        let expected = 1;

        tracing::trace!(actual, expected, "local leaf node count");

        actual == expected
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn merge() {
    let (_base_dir, repo) = setup().await;

    // Create remote branch and create a file in it.
    let remote_id = PublicKey::random();
    create_remote_file(&repo, remote_id, "test.txt", b"hello").await;

    let remote_vv = repo
        .get_branch(remote_id)
        .unwrap()
        .version_vector()
        .await
        .unwrap();

    // Open the local root.
    let local_branch = repo.local_branch().unwrap();
    let mut local_root = local_branch.open_or_create_root().await.unwrap();

    wait_for(&repo, || async {
        local_branch.version_vector().await.unwrap() > remote_vv
    })
    .await;

    local_root.refresh().await.unwrap();
    let content = local_root
        .lookup("test.txt")
        .unwrap()
        .file()
        .unwrap()
        .open()
        .await
        .unwrap()
        .read_to_end()
        .await
        .unwrap();
    assert_eq!(content, b"hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn recreate_previously_deleted_file() {
    let (_base_dir, repo) = setup().await;

    // Create file
    let mut file = repo.create_file("test.txt").await.unwrap();
    file.write_all(b"foo").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    // Read it back and check the content
    let content = read_file(&repo, "test.txt").await;
    assert_eq!(content, b"foo");

    // Delete it and assert it's gone
    repo.remove_entry("test.txt").await.unwrap();
    assert_matches!(repo.open_file("test.txt").await, Err(Error::EntryNotFound));

    // Create a file with the same name but different content
    let mut file = repo.create_file("test.txt").await.unwrap();
    file.write_all(b"bar").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    // Read it back and check the content
    let content = read_file(&repo, "test.txt").await;
    assert_eq!(content, b"bar");
}

#[tokio::test(flavor = "multi_thread")]
async fn recreate_previously_deleted_directory() {
    let (_base_dir, repo) = setup().await;

    // Create dir
    repo.create_directory("test").await.unwrap();

    // Check it exists
    assert_matches!(repo.open_directory("test").await, Ok(_));

    // Delete it and assert it's gone
    repo.remove_entry("test").await.unwrap();
    assert_matches!(repo.open_directory("test").await, Err(Error::EntryNotFound));

    // Create another directory with the same name
    repo.create_directory("test").await.unwrap();

    // Check it exists
    assert_matches!(repo.open_directory("test").await, Ok(_));
}

// This one used to deadlock
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_read_and_create_dir() {
    let (_base_dir, repo) = setup().await;

    let path = "/dir";
    let repo = Arc::new(repo);

    // The deadlock here happened because the reader lock when opening the directory is
    // acquired in the opposite order to the writer lock acqurired from flushing. I.e. the
    // reader lock acquires `/` and then `/dir`, but flushing acquires `/dir` first and
    // then `/`.
    let create_dir = scoped_task::spawn({
        let repo = repo.clone();
        async move {
            repo.create_directory(path).await.unwrap();
        }
    });

    let open_dir = scoped_task::spawn({
        let repo = repo.clone();
        let mut rx = repo.subscribe();

        async move {
            if let Ok(dir) = repo.open_directory(path).await {
                dir.entries().count();
                return;
            }

            wait_for_notification(&mut rx).await;
        }
    });

    create_dir.await.unwrap();
    open_dir.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_write_and_read_file() {
    let (_base_dir, repo) = setup().await;
    let repo = Arc::new(repo);

    let chunk_size = 1024;
    let chunk_count = 100;

    let write = scoped_task::spawn({
        let repo = repo.clone();

        async move {
            let mut file = repo.create_file("test.txt").await.unwrap();
            let mut buffer = vec![0; chunk_size];

            for _ in 0..chunk_count {
                rand::thread_rng().fill(&mut buffer[..]);
                file.write_all(&buffer).await.unwrap();
            }

            file.flush().await.unwrap();
        }
    });

    let read = scoped_task::spawn({
        let repo = repo.clone();
        let mut rx = repo.subscribe();

        async move {
            loop {
                match repo.open_file("test.txt").await {
                    Ok(file) => {
                        let actual_len = file.len();
                        let expected_len = (chunk_count * chunk_size) as u64;

                        if actual_len == expected_len {
                            break;
                        }
                    }
                    Err(Error::EntryNotFound) => (),
                    Err(error) => panic!("unexpected error: {:?}", error),
                };

                wait_for_notification(&mut rx).await;
            }
        }
    });

    write.await.unwrap();
    read.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn append_to_file() {
    let (_base_dir, repo) = setup().await;

    let mut file = repo.create_file("foo.txt").await.unwrap();
    file.write_all(b"foo").await.unwrap();
    file.flush().await.unwrap();

    // Concurrent file writes are currently not allowed so we need to drop the file before opening
    // it again.
    drop(file);

    let mut file = repo.open_file("foo.txt").await.unwrap();
    file.seek(SeekFrom::End(0));
    file.write_all(b"bar").await.unwrap();
    file.flush().await.unwrap();

    let mut file = repo.open_file("foo.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"foobar");
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_onto_non_existing_entry() {
    let (_base_dir, repo) = setup().await;

    repo.create_file("src.txt").await.unwrap();
    repo.move_entry("/", "src.txt", "/", "dst.txt")
        .await
        .unwrap();

    assert_matches!(repo.open_file("src.txt").await, Err(Error::EntryNotFound));
    assert_matches!(repo.open_file("dst.txt").await, Ok(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_onto_tombstone() {
    let (_base_dir, repo) = setup().await;

    repo.create_file("src.txt").await.unwrap();

    repo.create_file("dst.txt").await.unwrap();
    repo.remove_entry("dst.txt").await.unwrap();

    repo.move_entry("/", "src.txt", "/", "dst.txt")
        .await
        .unwrap();

    assert_matches!(repo.open_file("src.txt").await, Err(Error::EntryNotFound));
    assert_matches!(repo.open_file("dst.txt").await, Ok(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_onto_existing_file() {
    let (_base_dir, repo) = setup().await;

    let mut file = repo.create_file("src.txt").await.unwrap();
    file.write_all(b"src").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    let mut file = repo.create_file("dst.txt").await.unwrap();
    file.write_all(b"dst").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    repo.move_entry("/", "src.txt", "/", "dst.txt")
        .await
        .unwrap();

    assert_matches!(repo.open_file("src.txt").await, Err(Error::EntryNotFound));

    let mut file = repo.open_file("dst.txt").await.unwrap();
    assert_eq!(file.read_to_end().await.unwrap(), b"src");
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_onto_existing_directory() {
    let (_base_dir, repo) = setup().await;

    repo.create_file("src.txt").await.unwrap();
    repo.create_directory("dst").await.unwrap();

    assert_matches!(
        repo.move_entry("/", "src.txt", "/", "dst").await,
        Err(Error::EntryIsDirectory)
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn move_directory_onto_non_existing_entry() {
    let (_base_dir, repo) = setup().await;

    repo.create_directory("src").await.unwrap();
    repo.move_entry("/", "src", "/", "dst").await.unwrap();

    assert_matches!(repo.open_directory("src").await, Err(Error::EntryNotFound));
    assert_matches!(repo.open_directory("dst").await, Ok(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_directory_onto_file_tombstone() {
    let (_base_dir, repo) = setup().await;

    repo.create_directory("src").await.unwrap();
    repo.create_file("dst").await.unwrap();
    repo.remove_entry("dst").await.unwrap();

    repo.move_entry("/", "src", "/", "dst").await.unwrap();

    assert_matches!(repo.open_directory("src").await, Err(Error::EntryNotFound));
    assert_matches!(repo.open_directory("dst").await, Ok(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_directory_onto_directory_tombstone() {
    let (_base_dir, repo) = setup().await;

    repo.create_directory("src").await.unwrap();
    repo.create_directory("dst").await.unwrap();
    repo.remove_entry("dst").await.unwrap();

    repo.move_entry("/", "src", "/", "dst").await.unwrap();

    assert_matches!(repo.open_directory("src").await, Err(Error::EntryNotFound));
    assert_matches!(repo.open_directory("dst").await, Ok(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_directory_onto_existing_empty_directory() {
    let (_base_dir, repo) = setup().await;

    repo.create_directory("src").await.unwrap();
    repo.create_directory("dst").await.unwrap();

    repo.move_entry("/", "src", "/", "dst").await.unwrap();

    assert_matches!(repo.open_directory("src").await, Err(Error::EntryNotFound));
    assert_matches!(repo.open_directory("dst").await, Ok(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_directory_onto_existing_non_empty_directory() {
    let (_base_dir, repo) = setup().await;

    repo.create_directory("src").await.unwrap();

    repo.create_directory("dst").await.unwrap();
    repo.create_file("dst/file.txt").await.unwrap();

    assert_matches!(
        repo.move_entry("/", "src", "/", "dst").await,
        Err(Error::DirectoryNotEmpty)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn move_directory_onto_existing_file() {
    let (_base_dir, repo) = setup().await;

    repo.create_directory("src").await.unwrap();
    repo.create_file("dst").await.unwrap();

    assert_matches!(
        repo.move_entry("/", "src", "/", "dst").await,
        Err(Error::EntryIsFile)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_into_non_existing_directory() {
    let (_base_dir, repo) = setup().await;

    repo.create_file("src.txt").await.unwrap();

    assert_matches!(
        repo.move_entry("/", "src.txt", "/missing", "dst.txt").await,
        Err(Error::EntryNotFound)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_open_file() {
    let (_base_dir, repo) = setup().await;

    let _file = repo.create_file("foo.txt").await.unwrap();

    repo.remove_entry("foo.txt").await.unwrap();
    assert_matches!(repo.open_file("foo.txt").await, Err(Error::EntryNotFound));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_from_open_file() {
    let (_base_dir, repo) = setup().await;

    let _file = repo.create_file("src.txt").await.unwrap();

    repo.move_entry("/", "src.txt", "/", "dst.txt")
        .await
        .unwrap();

    assert_matches!(repo.open_file("src.txt").await, Err(Error::EntryNotFound));
    assert_matches!(repo.open_file("dst.txt").await, Ok(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_onto_open_file() {
    let (_base_dir, repo) = setup().await;

    let mut file = repo.create_file("src.txt").await.unwrap();
    file.write(b"src").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    let mut file = repo.create_file("dst.txt").await.unwrap();
    file.write(b"dst").await.unwrap();
    file.flush().await.unwrap();

    repo.move_entry("/", "src.txt", "/", "dst.txt")
        .await
        .unwrap();

    assert_matches!(repo.open_file("src.txt").await, Err(Error::EntryNotFound));

    let mut file = repo.open_file("dst.txt").await.unwrap();
    assert_eq!(file.read_to_end().await.unwrap(), b"src");
}

// TODO: test reading / writing file that's been removed / moved from / moved onto

#[tokio::test(flavor = "multi_thread")]
async fn blind_access_non_empty_repo() {
    test_utils::init_log();

    let (_base_dir, pool) = db::create_temp().await.unwrap();
    let params =
        RepositoryParams::with_pool(pool, "test").with_parent_monitor(StateMonitor::make_root());
    let local_secret = LocalSecret::random();

    // Create the repo and put a file in it.
    let repo = Repository::create(
        &params,
        Access::WriteLocked {
            local_read_secret: local_secret.clone(),
            local_write_secret: local_secret,
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    let mut file = repo.create_file("secret.txt").await.unwrap();
    file.write_all(b"redacted").await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    // Reopen the repo first explicitly in blind mode and then using incorrect secret. The two ways
    // should be indistinguishable from each other.
    for (local_secret, access_mode) in [
        (None, AccessMode::Blind),
        (Some(LocalSecret::random()), AccessMode::Write),
    ] {
        // Reopen the repo in blind mode.
        let repo = Repository::open(&params, local_secret.clone(), access_mode)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "Repo should open in blind mode (local_secret.is_some:{:?})",
                    local_secret.is_some(),
                )
            });

        // Reading files is not allowed.
        assert_matches!(
            repo.open_file("secret.txt").await,
            Err(Error::PermissionDenied)
        );

        // Creating files is not allowed.
        assert_matches!(
            repo.create_file("hack.txt").await,
            Err(Error::PermissionDenied)
        );

        // Removing files is not allowed.
        assert_matches!(
            repo.remove_entry("secret.txt").await,
            Err(Error::PermissionDenied)
        );

        // Reading the root directory is not allowed either.
        assert_matches!(repo.open_directory("/").await, Err(Error::PermissionDenied));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blind_access_empty_repo() {
    test_utils::init_log();

    let (_base_dir, pool) = db::create_temp().await.unwrap();
    let params = RepositoryParams::with_pool(pool, "test");

    let local_secret = LocalSecret::random();

    // Create an empty repo.
    Repository::create(
        &params,
        Access::WriteLocked {
            local_read_secret: local_secret.clone(),
            local_write_secret: local_secret,
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    // Reopen the repo in blind mode.
    let repo = Repository::open(&params, Some(LocalSecret::random()), AccessMode::Read)
        .await
        .unwrap();

    // Reading the root directory is not allowed.
    assert_matches!(repo.open_directory("/").await, Err(Error::PermissionDenied));
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_same_replica() {
    test_utils::init_log();

    let (_base_dir, pool) = db::create_temp().await.unwrap();
    let params = RepositoryParams::with_pool(pool, "test");

    let repo = Repository::create(
        &params,
        Access::WriteUnlocked {
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write_all(b"hello world").await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    // Reopen the repo in read-only mode.
    let repo = Repository::open(&params, None, AccessMode::Read)
        .await
        .unwrap();

    // Reading files is allowed.
    let mut file = repo.open_file("public.txt").await.unwrap();

    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"hello world");

    // Writing is not allowed.
    file.seek(SeekFrom::Start(0));
    // short writes that don't cross block boundaries don't trigger the permission check which is
    // why the following works...
    file.write_all(b"hello universe").await.unwrap();
    // ...but flushing the file is not allowed.
    assert_matches!(file.flush().await, Err(Error::PermissionDenied));

    drop(file);

    // Creating files is not allowed.
    assert_matches!(
        repo.create_file("hack.txt").await,
        Err(Error::PermissionDenied)
    );

    // Removing files is not allowed.
    assert_matches!(
        repo.remove_entry("public.txt").await,
        Err(Error::PermissionDenied)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_different_replica() {
    test_utils::init_log();

    let (_base_dir, pool) = db::create_temp().await.unwrap();

    let params_a = RepositoryParams::with_pool(pool.clone(), "test").with_device_id(rand::random());
    let repo = Repository::create(
        &params_a,
        Access::WriteUnlocked {
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write_all(b"hello world").await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    let params_b = RepositoryParams::with_pool(pool, "test").with_device_id(rand::random());
    let repo = Repository::open(&params_b, None, AccessMode::Read)
        .await
        .unwrap();

    let mut file = repo.open_file("public.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"hello world");
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_forked_remote_file() {
    let (_base_dir, repo) = setup().await;

    let remote_id = PublicKey::random();
    tracing::info!(local_id = ?repo.local_branch().unwrap().id(), ?remote_id);

    create_remote_file(&repo, remote_id, "test.txt", b"foo").await;

    let local_branch = repo.local_branch().unwrap();
    let mut file = repo.open_file("test.txt").await.unwrap();
    file.fork(local_branch).await.unwrap();
    file.truncate(0).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_create_file() {
    let (_base_dir, repo) = setup().await;
    let local_branch = repo.local_branch().unwrap();

    let root_vv_0 = local_branch.version_vector().await.unwrap();

    let mut file = repo.create_file("parent/test.txt").await.unwrap();

    let root_vv_1 = file
        .parent()
        .await
        .unwrap()
        .parent()
        .await
        .unwrap()
        .unwrap()
        .version_vector()
        .await
        .unwrap();
    let parent_vv_1 = file.parent().await.unwrap().version_vector().await.unwrap();
    let file_vv_1 = file.version_vector().await.unwrap();

    assert!(root_vv_1 > root_vv_0);

    file.write_all(b"blah").await.unwrap();
    file.flush().await.unwrap();

    let root_vv_2 = file
        .parent()
        .await
        .unwrap()
        .parent()
        .await
        .unwrap()
        .unwrap()
        .version_vector()
        .await
        .unwrap();
    let parent_vv_2 = file.parent().await.unwrap().version_vector().await.unwrap();
    let file_vv_2 = file.version_vector().await.unwrap();

    assert!(root_vv_2 > root_vv_1);
    assert!(parent_vv_2 > parent_vv_1);
    assert!(file_vv_2 > file_vv_1);
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_deep_hierarchy() {
    let (_base_dir, repo) = setup().await;
    let local_branch = repo.local_branch().unwrap();
    let local_id = *local_branch.id();

    let depth = 10;
    let mut dirs = Vec::new();
    dirs.push(local_branch.open_or_create_root().await.unwrap());

    for i in 0..depth {
        let dir = dirs
            .last_mut()
            .unwrap()
            .create_directory(format!("dir-{}", i))
            .await
            .unwrap();
        dirs.push(dir);
    }

    // Each directory's local version is one less than its parent.
    for (index, dir) in dirs.iter().skip(1).enumerate() {
        assert_eq!(
            dir.version_vector().await.unwrap(),
            vv![local_id => (depth - index) as u64]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_recreate_deleted_file() {
    let (_base_dir, repo) = setup().await;

    let local_id = *repo.local_branch().unwrap().id();

    let file = repo.create_file("test.txt").await.unwrap();
    drop(file);

    repo.remove_entry("test.txt").await.unwrap();

    let file = repo.create_file("test.txt").await.unwrap();
    assert_eq!(
        file.version_vector().await.unwrap(),
        vv![local_id => 3 /* = 1*create + 1*remove + 1*create again */]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_fork() {
    use tokio::time;

    // TODO: this test would be more precise without merger. Consider converting it to a
    // joint_directory test.

    let (_base_dir, repo) = setup().await;

    let local_branch = repo.local_branch().unwrap();
    let remote_branch = repo
        .get_branch(PublicKey::random())
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    tracing::info!(local_id = ?local_branch.id(), remote_id = ?remote_branch.id());

    time::timeout(Duration::from_secs(5), async move {
        let mut remote_root = remote_branch.open_or_create_root().await.unwrap();
        let mut remote_parent = remote_root.create_directory("parent".into()).await.unwrap();
        let mut file = create_file_in_directory(&mut remote_parent, "foo.txt", &[]).await;

        remote_parent.refresh().await.unwrap();
        let remote_parent_vv = remote_parent.version_vector().await.unwrap();
        let remote_file_vv = file.version_vector().await.unwrap();

        tracing::info!("step 1");
        file.fork(local_branch.clone()).await.unwrap();
        tracing::info!("step 2");

        let local_file_vv_0 = file.version_vector().await.unwrap();
        assert_eq!(local_file_vv_0, remote_file_vv);

        let local_parent_vv_0 = local_branch
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("parent")
            .unwrap()
            .version_vector()
            .clone();

        assert!(local_parent_vv_0 <= remote_parent_vv);

        drop(file);

        // modify the file and fork again
        let mut file = remote_parent
            .lookup("foo.txt")
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();
        file.write_all(b"hello").await.unwrap();
        file.flush().await.unwrap();

        remote_parent.refresh().await.unwrap();
        let remote_parent_vv = remote_parent.version_vector().await.unwrap();
        let remote_file_vv = file.version_vector().await.unwrap();

        file.fork(local_branch.clone()).await.unwrap();

        let local_file_vv_1 = file.version_vector().await.unwrap();
        assert_eq!(local_file_vv_1, remote_file_vv);
        assert!(local_file_vv_1 > local_file_vv_0);

        let local_parent_vv_1 = local_branch
            .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
            .await
            .unwrap()
            .lookup("parent")
            .unwrap()
            .version_vector()
            .clone();

        assert!(local_parent_vv_1 <= remote_parent_vv);
        assert!(local_parent_vv_1 >= local_parent_vv_0);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_empty_directory() {
    let (_base_dir, repo) = setup().await;

    let local_branch = repo.local_branch().unwrap();
    let local_id = *local_branch.id();

    let dir = repo.create_directory("stuff").await.unwrap();
    assert_eq!(dir.version_vector().await.unwrap(), vv![local_id => 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_moved_non_empty_directory() {
    let (_base_dir, repo) = setup().await;

    repo.create_directory("foo").await.unwrap();
    repo.create_file("foo/stuff.txt").await.unwrap();

    let vv_0 = repo
        .local_branch()
        .unwrap()
        .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
        .await
        .unwrap()
        .lookup("foo")
        .unwrap()
        .version_vector()
        .clone();

    repo.move_entry("/", "foo", "/", "bar").await.unwrap();

    let vv_1 = repo
        .local_branch()
        .unwrap()
        .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
        .await
        .unwrap()
        .lookup("bar")
        .unwrap()
        .version_vector()
        .clone();

    assert!(vv_1 > vv_0);
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_file_moved_over_tombstone() {
    let (_base_dir, repo) = setup().await;

    let mut file = repo.create_file("old.txt").await.unwrap();
    file.write_all(b"a").await.unwrap();
    file.flush().await.unwrap();

    let vv_0 = file.version_vector().await.unwrap();

    drop(file);
    repo.remove_entry("old.txt").await.unwrap();

    // The tombstone's vv is the original vv incremented by 1.
    let branch_id = *repo.local_branch().unwrap().id();
    let vv_1 = vv_0.incremented(branch_id);

    repo.create_file("new.txt").await.unwrap();
    repo.move_entry("/", "new.txt", "/", "old.txt")
        .await
        .unwrap();

    let vv_2 = repo
        .local_branch()
        .unwrap()
        .open_root(DirectoryLocking::Enabled, DirectoryFallback::Disabled)
        .await
        .unwrap()
        .lookup("old.txt")
        .unwrap()
        .version_vector()
        .clone();

    assert!(vv_2 > vv_1);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_conflict_modify_local() {
    let (_base_dir, repo) = setup().await;

    let local_branch = repo.local_branch().unwrap();
    let local_id = *local_branch.id();

    let remote_id = PublicKey::random();
    let remote_branch = repo
        .get_branch(remote_id)
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    // Create two concurrent versions of the same file.
    let local_file = create_file_in_branch(&local_branch, "test.txt", b"local v1").await;

    assert_eq!(
        local_file.version_vector().await.unwrap(),
        vv![local_id => 2]
    );
    drop(local_file);

    let remote_file = create_file_in_branch(&remote_branch, "test.txt", b"remote v1").await;
    assert_eq!(
        remote_file.version_vector().await.unwrap(),
        vv![remote_id => 2]
    );
    drop(remote_file);

    // Modify the local version.
    let mut local_file = repo.open_file_version("test.txt", &local_id).await.unwrap();
    local_file.write_all(b"local v2").await.unwrap();
    local_file.flush().await.unwrap();
    drop(local_file);

    let mut local_file = repo.open_file_version("test.txt", &local_id).await.unwrap();
    assert_eq!(local_file.read_to_end().await.unwrap(), b"local v2");
    assert_eq!(
        local_file.version_vector().await.unwrap(),
        vv![local_id => 3]
    );

    let mut remote_file = repo
        .open_file_version("test.txt", &remote_id)
        .await
        .unwrap();
    assert_eq!(remote_file.read_to_end().await.unwrap(), b"remote v1");
    assert_eq!(
        remote_file.version_vector().await.unwrap(),
        vv![remote_id => 2]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn file_conflict_attempt_to_fork_and_modify_remote() {
    let (_base_dir, repo) = setup().await;

    let local_branch = repo.local_branch().unwrap();

    let remote_id = PublicKey::random();
    let remote_branch = repo
        .get_branch(remote_id)
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    // Create two concurrent versions of the same file.
    create_file_in_branch(&local_branch, "test.txt", b"local v1").await;
    create_file_in_branch(&remote_branch, "test.txt", b"remote v1").await;

    // Attempt to fork the remote version (fork is required to modify it)
    let mut remote_file = repo
        .open_file_version("test.txt", &remote_id)
        .await
        .unwrap();
    assert_matches!(
        remote_file.fork(local_branch).await,
        Err(Error::EntryExists)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn size() {
    let (_base_dir, repo) = setup().await;
    assert_eq!(repo.size().await.unwrap().to_bytes(), 0);

    let mut file = repo.create_file("test.txt").await.unwrap();

    // 2 blocks: 1 for the file and 1 for the root dir
    assert_eq!(
        repo.size().await.unwrap().to_bytes(),
        2 * (BLOCK_SIZE + BlockId::SIZE + BLOCK_NONCE_SIZE) as u64
    );

    // 1 block size + 1 byte == 2 blocks
    let content = random_bytes(BLOCK_SIZE - blob::HEADER_SIZE + 1);
    file.write_all(&content).await.unwrap();
    file.flush().await.unwrap();

    // 3 blocks: 2 for the file and 1 for the root dir
    assert_eq!(
        repo.size().await.unwrap().to_bytes(),
        3 * (BLOCK_SIZE + BlockId::SIZE + BLOCK_NONCE_SIZE) as u64
    );
}

async fn setup() -> (TempDir, Repository) {
    test_utils::init_log();

    let base_dir = TempDir::new().unwrap();
    let repo = Repository::create(
        &RepositoryParams::new(base_dir.path().join("repo.db")),
        Access::WriteUnlocked {
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    (base_dir, repo)
}

async fn read_file(repo: &Repository, path: impl AsRef<Utf8Path>) -> Vec<u8> {
    let mut file = repo.open_file(path).await.unwrap();
    file.read_to_end().await.unwrap()
}

#[instrument(skip(repo, content), fields(content.len = content.len()))]
async fn create_remote_file(
    repo: &Repository,
    remote_id: PublicKey,
    name: &str,
    content: &[u8],
) -> File {
    let remote_branch = repo
        .get_branch(remote_id)
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    create_file_in_branch(&remote_branch, name, content).await
}

async fn create_file_in_branch(branch: &Branch, name: &str, content: &[u8]) -> File {
    let mut root = branch.open_or_create_root().await.unwrap();
    create_file_in_directory(&mut root, name, content).await
}

async fn create_file_in_directory(dir: &mut Directory, name: &str, content: &[u8]) -> File {
    let mut file = dir.create_file(name.into()).await.unwrap();
    file.write_all(content).await.unwrap();
    file.flush().await.unwrap();
    file
}

fn random_bytes(size: usize) -> Vec<u8> {
    let mut buffer = vec![0; size];
    rand::thread_rng().fill(&mut buffer[..]);
    buffer
}

async fn wait_for_notification(rx: &mut Receiver<Event>) {
    match timeout(Duration::from_secs(5), rx.recv()).await {
        Ok(Ok(_)) => (),
        Ok(Err(RecvError::Lagged(_))) => (),
        Ok(Err(RecvError::Closed)) => {
            panic!("notification channel unexpectedly closed")
        }
        Err(_) => panic!("timeout waiting for notification"),
    }
}

async fn wait_for<F, Fut>(repo: &Repository, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let mut rx = repo.subscribe();

    time::timeout(Duration::from_secs(10), async {
        loop {
            if f().await {
                break;
            }

            wait_for_notification(&mut rx).await
        }
    })
    .await
    .expect("timeout waiting for condition")
}

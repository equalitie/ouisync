use super::*;
use crate::{blob, block::BLOCK_SIZE, crypto::cipher::SecretKey, db, WriteSecrets};
use assert_matches::assert_matches;
use rand::Rng;
use std::io::SeekFrom;
use tempfile::TempDir;
use tokio::time::{sleep, timeout, Duration};

#[tokio::test(flavor = "multi_thread")]
async fn root_directory_always_exists() {
    let (_base_dir, repo) = setup().await;
    let _ = repo.open_directory("/").await.unwrap();
}

// Count leaf nodes in the index of the local branch.
async fn count_local_index_leaf_nodes(repo: &Repository) -> usize {
    let store = repo.store();
    let branch = repo.local_branch().unwrap();
    let mut conn = store.db().acquire().await.unwrap();
    branch.data().count_leaf_nodes(&mut conn).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn count_leaf_nodes_sanity_checks() {
    let (_base_dir, repo) = setup().await;

    let file_name = "test.txt";

    //------------------------------------------------------------------------
    // Create a small file in the root.

    let mut file = repo.create_file(file_name).await.unwrap();
    file.write(&random_bytes(BLOCK_SIZE - blob::HEADER_SIZE))
        .await
        .unwrap();
    file.flush().await.unwrap();

    // 2 = one for the root + one for the file.
    assert_eq!(count_local_index_leaf_nodes(&repo).await, 2);

    //------------------------------------------------------------------------
    // Make the file bigger to expand to two blocks

    file.write(&random_bytes(1)).await.unwrap();
    file.flush().await.unwrap();

    // 3 = one for the root + two for the file.
    assert_eq!(count_local_index_leaf_nodes(&repo).await, 3);

    //------------------------------------------------------------------------
    // Remove the file, we should end up with just one block for the root.

    drop(file);
    repo.remove_entry(file_name).await.unwrap();
    repo.force_work().await.unwrap(); // run the garbage collector

    // 1 = one for the root with a tombstone entry
    assert_eq!(count_local_index_leaf_nodes(&repo).await, 1);

    //------------------------------------------------------------------------
}

#[tokio::test(flavor = "multi_thread")]
async fn merge() {
    let (_base_dir, repo) = setup().await;

    // Create remote branch and create a file in it.
    let remote_id = PublicKey::random();
    create_remote_file(&repo, remote_id, "test.txt", b"hello").await;

    // Open the local root.
    let local_branch = repo.local_branch().unwrap();
    let mut local_root = local_branch.open_or_create_root().await.unwrap();

    repo.force_work().await.unwrap();

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
    file.write(b"foo").await.unwrap();
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
    file.write(b"bar").await.unwrap();
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

            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => (),
                Err(RecvError::Closed) => panic!("Event channel unexpectedly closed"),
            }
        }
    });

    timeout(Duration::from_secs(5), async {
        create_dir.await.unwrap();
        open_dir.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_write_and_read_file() {
    let (_base_dir, repo) = setup().await;
    let repo = Arc::new(repo);

    let chunk_size = 1024;
    let chunk_count = 100;

    let write_file = scoped_task::spawn({
        let repo = repo.clone();

        async move {
            let mut file = repo.create_file("test.txt").await.unwrap();
            let mut buffer = vec![0; chunk_size];

            for _ in 0..chunk_count {
                rand::thread_rng().fill(&mut buffer[..]);
                file.write(&buffer).await.unwrap();
            }

            file.flush().await.unwrap();
        }
    });

    let open_dir = scoped_task::spawn({
        let repo = repo.clone();

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

                sleep(Duration::from_millis(10)).await;
            }
        }
    });

    timeout(Duration::from_secs(5), async {
        write_file.await.unwrap();
        open_dir.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn append_to_file() {
    let (_base_dir, repo) = setup().await;

    let mut file = repo.create_file("foo.txt").await.unwrap();
    file.write(b"foo").await.unwrap();
    file.flush().await.unwrap();

    // Concurrent file writes are currently not allowed so we need to drop the file before opening
    // it again.
    drop(file);

    let mut file = repo.open_file("foo.txt").await.unwrap();
    file.seek(SeekFrom::End(0)).await.unwrap();
    file.write(b"bar").await.unwrap();
    file.flush().await.unwrap();

    let mut file = repo.open_file("foo.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"foobar");
}

#[tokio::test(flavor = "multi_thread")]
async fn blind_access_non_empty_repo() {
    let (_base_dir, pool) = db::create_temp().await.unwrap();
    let device_id = rand::random();

    let local_key = SecretKey::random();
    // Create the repo and put a file in it.
    let repo = Repository::create(
        RepositoryDb::new(pool.clone()),
        device_id,
        Access::WriteLocked {
            local_read_key: local_key.clone(),
            local_write_key: local_key,
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    let mut file = repo.create_file("secret.txt").await.unwrap();
    file.write(b"redacted").await.unwrap();
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
        let repo = Repository::open_in(pool.clone(), device_id, local_secret.clone(), access_mode)
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
    let (_base_dir, pool) = db::create_temp().await.unwrap();
    let device_id = rand::random();

    let local_key = SecretKey::random();

    // Create an empty repo.
    Repository::create(
        RepositoryDb::new(pool.clone()),
        device_id,
        Access::WriteLocked {
            local_read_key: local_key.clone(),
            local_write_key: local_key,
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    // Reopen the repo in blind mode.
    let repo = Repository::open_in(
        pool.clone(),
        device_id,
        Some(LocalSecret::random()),
        AccessMode::Read,
    )
    .await
    .unwrap();

    // Reading the root directory is not allowed.
    assert_matches!(repo.open_directory("/").await, Err(Error::PermissionDenied));
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_same_replica() {
    let (_base_dir, pool) = db::create_temp().await.unwrap();
    let device_id = rand::random();

    let repo = Repository::create(
        RepositoryDb::new(pool.clone()),
        device_id,
        Access::WriteUnlocked {
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write(b"hello world").await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    // Reopen the repo in read-only mode.
    let repo = Repository::open_in(pool, device_id, None, AccessMode::Read)
        .await
        .unwrap();

    // Reading files is allowed.
    let mut file = repo.open_file("public.txt").await.unwrap();

    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"hello world");

    // Writing is not allowed.
    file.seek(SeekFrom::Start(0)).await.unwrap();
    // short writes that don't cross block boundaries don't trigger the permission check which is
    // why the following works...
    file.write(b"hello universe").await.unwrap();
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
    let (_base_dir, pool) = db::create_temp().await.unwrap();

    let device_id_a = rand::random();
    let repo = Repository::create(
        RepositoryDb::new(pool.clone()),
        device_id_a,
        Access::WriteUnlocked {
            secrets: WriteSecrets::random(),
        },
    )
    .await
    .unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write(b"hello world").await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    let device_id_b = rand::random();
    let repo = Repository::open_in(pool, device_id_b, None, AccessMode::Read)
        .await
        .unwrap();

    let mut file = repo.open_file("public.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"hello world");
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_forked_remote_file() {
    init_log();
    let (_base_dir, repo) = setup().await;

    create_remote_file(&repo, PublicKey::random(), "test.txt", b"foo").await;

    let local_branch = repo.local_branch().unwrap();
    let mut file = repo.open_file("test.txt").await.unwrap();
    file.fork(local_branch).await.unwrap();
    file.truncate(0).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_modify_remote_file() {
    let (_base_dir, repo) = setup().await;
    let remote_id = PublicKey::random();

    create_remote_file(&repo, remote_id, "test.txt", b"foo").await;

    let remote_branch = repo.get_branch(remote_id).unwrap();

    let mut file = remote_branch
        .open_root(MissingBlockStrategy::Fail)
        .await
        .unwrap()
        .lookup("test.txt")
        .unwrap()
        .file()
        .unwrap()
        .open()
        .await
        .unwrap();

    file.truncate(0).await.unwrap();
    assert_matches!(file.flush().await, Err(Error::PermissionDenied));

    file.write(b"bar").await.unwrap();
    assert_matches!(file.flush().await, Err(Error::PermissionDenied));
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

    file.write(b"blah").await.unwrap();
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
    init_log();
    let (_base_dir, repo) = setup().await;

    let local_branch = repo.local_branch().unwrap();

    let remote_branch = repo
        .get_branch(PublicKey::random())
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    let mut remote_root = remote_branch.open_or_create_root().await.unwrap();
    let mut remote_parent = remote_root.create_directory("parent".into()).await.unwrap();
    let mut file = create_file_in_directory(&mut remote_parent, "foo.txt", &[]).await;

    remote_parent.refresh().await.unwrap();
    let remote_parent_vv = remote_parent.version_vector().await.unwrap();
    let remote_file_vv = file.version_vector().await.unwrap();

    file.fork(local_branch.clone()).await.unwrap();

    let local_file_vv_0 = file.version_vector().await.unwrap();
    assert_eq!(local_file_vv_0, remote_file_vv);

    let local_parent_vv_0 = local_branch
        .open_root(MissingBlockStrategy::Fail)
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
    file.write(b"hello").await.unwrap();
    file.flush().await.unwrap();

    remote_parent.refresh().await.unwrap();
    let remote_parent_vv = remote_parent.version_vector().await.unwrap();
    let remote_file_vv = file.version_vector().await.unwrap();

    file.fork(local_branch.clone()).await.unwrap();

    let local_file_vv_1 = file.version_vector().await.unwrap();
    assert_eq!(local_file_vv_1, remote_file_vv);
    assert!(local_file_vv_1 > local_file_vv_0);

    let local_parent_vv_1 = local_branch
        .open_root(MissingBlockStrategy::Fail)
        .await
        .unwrap()
        .lookup("parent")
        .unwrap()
        .version_vector()
        .clone();

    assert!(local_parent_vv_1 <= remote_parent_vv);
    assert!(local_parent_vv_1 > local_parent_vv_0);
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
        .open_root(MissingBlockStrategy::Fail)
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
        .open_root(MissingBlockStrategy::Fail)
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
    file.write(b"a").await.unwrap();
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
        .open_root(MissingBlockStrategy::Fail)
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
    local_file.write(b"local v2").await.unwrap();
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

async fn setup() -> (TempDir, Repository) {
    let base_dir = TempDir::new().unwrap();
    let repo = Repository::create(
        RepositoryDb::create(base_dir.path().join("repo.db"))
            .await
            .unwrap(),
        rand::random(),
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
    file.write(content).await.unwrap();
    file.flush().await.unwrap();
    file
}

fn random_bytes(size: usize) -> Vec<u8> {
    let mut buffer = vec![0; size];
    rand::thread_rng().fill(&mut buffer[..]);
    buffer
}

#[allow(unused)]
fn init_log() {
    use tracing::metadata::LevelFilter;

    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        .with_test_writer()
        .try_init()
        .ok();
}

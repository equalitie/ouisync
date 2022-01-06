use std::io::SeekFrom;

use super::*;
use crate::db;
use assert_matches::assert_matches;
use tokio::time::{sleep, Duration};

#[tokio::test(flavor = "multi_thread")]
async fn root_directory_always_exists() {
    let writer_id = rand::random();
    let repo = Repository::create(
        &db::Store::Memory,
        writer_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();
    let _ = repo.open_directory("/").await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn merge() {
    let repo = Repository::create(
        &db::Store::Memory,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        true,
    )
    .await
    .unwrap();

    // Create remote branch and create a file in it.
    let remote_id = PublicKey::random();
    let remote_branch = repo.create_remote_branch(remote_id).await.unwrap();
    let remote_root = remote_branch.open_or_create_root().await.unwrap();

    let mut file = remote_root
        .create_file("test.txt".to_owned())
        .await
        .unwrap();
    file.write(b"hello", &remote_branch).await.unwrap();
    file.flush().await.unwrap();

    // Open the local root.
    let local_branch = repo.local_branch().await.unwrap();
    let local_root = local_branch.open_or_create_root().await.unwrap();

    let mut rx = repo.subscribe();

    loop {
        match local_root
            .read()
            .await
            .lookup_version("test.txt", &remote_id)
        {
            Ok(entry) => {
                let content = entry
                    .file()
                    .unwrap()
                    .open()
                    .await
                    .unwrap()
                    .read_to_end()
                    .await
                    .unwrap();
                assert_eq!(content, b"hello");
                break;
            }
            Err(Error::EntryNotFound) => (),
            Err(error) => panic!("unexpected error: {:?}", error),
        }

        rx.recv().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn recreate_previously_deleted_file() {
    let local_id = rand::random();
    let repo = Repository::create(
        &db::Store::Memory,
        local_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.local_branch().await.unwrap();

    // Create file
    let mut file = repo.create_file("test.txt").await.unwrap();
    file.write(b"foo", &local_branch).await.unwrap();
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
    file.write(b"bar", &local_branch).await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    // Read it back and check the content
    let content = read_file(&repo, "test.txt").await;
    assert_eq!(content, b"bar");
}

#[tokio::test(flavor = "multi_thread")]
async fn recreate_previously_deleted_directory() {
    let local_id = rand::random();
    let repo = Repository::create(
        &db::Store::Memory,
        local_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    // Create dir
    repo.create_directory("test")
        .await
        .unwrap()
        .flush(None)
        .await
        .unwrap();

    // Check it exists
    assert_matches!(repo.open_directory("test").await, Ok(_));

    // Delete it and assert it's gone
    repo.remove_entry("test").await.unwrap();
    assert_matches!(repo.open_directory("test").await, Err(Error::EntryNotFound));

    // Create another directory with the same name
    repo.create_directory("test")
        .await
        .unwrap()
        .flush(None)
        .await
        .unwrap();

    // Check it exists
    assert_matches!(repo.open_directory("test").await, Ok(_))
}

// This one used to deadlock
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_read_and_create_dir() {
    let writer_id = rand::random();
    let repo = Repository::create(
        &db::Store::Memory,
        writer_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let path = "/dir";
    let repo = Arc::new(repo);

    let _watch_dog = scoped_task::spawn(async {
        sleep(Duration::from_millis(5 * 1000)).await;
        panic!("timed out");
    });

    // The deadlock here happened because the reader lock when opening the directory is
    // acquired in the opposite order to the writer lock acqurired from flushing. I.e. the
    // reader lock acquires `/` and then `/dir`, but flushing acquires `/dir` first and
    // then `/`.
    let create_dir = scoped_task::spawn({
        let repo = repo.clone();
        async move {
            let dir = repo.create_directory(path).await.unwrap();
            dir.flush(None).await.unwrap();
        }
    });

    let open_dir = scoped_task::spawn({
        let repo = repo.clone();
        async move {
            for _ in 1..10 {
                if let Ok(dir) = repo.open_directory(path).await {
                    dir.read().await.entries().count();
                    return;
                }
                // Sometimes opening the directory may outrace its creation,
                // so continue to retry again.
            }
            panic!("Failed to open the directory after multiple attempts");
        }
    });

    create_dir.await.unwrap();
    open_dir.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn append_to_file() {
    let repo = Repository::create(
        &db::Store::Memory,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.local_branch().await.unwrap();
    let mut file = repo.create_file("foo.txt").await.unwrap();
    file.write(b"foo", &local_branch).await.unwrap();
    file.flush().await.unwrap();

    let mut file = repo.open_file("foo.txt").await.unwrap();
    file.seek(SeekFrom::End(0)).await.unwrap();
    file.write(b"bar", &local_branch).await.unwrap();
    file.flush().await.unwrap();

    let mut file = repo.open_file("foo.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"foobar");
}

#[tokio::test(flavor = "multi_thread")]
async fn blind_access() {
    let pool = db::Pool::connect(":memory:").await.unwrap();
    let replica_id = rand::random();

    // Create the repo and put a file in it.
    let repo = Repository::create_in(
        pool.clone(),
        replica_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.local_branch().await.unwrap();

    let mut file = repo.create_file("secret.txt").await.unwrap();
    file.write(b"redacted", &local_branch).await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    // Reopen the repo in blind mode.
    let repo = Repository::open_in(pool, replica_id, None, AccessMode::Blind, false)
        .await
        .unwrap();

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
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_same_replica() {
    let pool = db::Pool::connect(":memory:").await.unwrap();
    let replica_id = rand::random();
    let master_secret = MasterSecret::random();

    let repo = Repository::create_in(
        pool.clone(),
        replica_id,
        master_secret.clone(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.local_branch().await.unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write(b"hello world", &local_branch).await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    // Reopen the repo in read-only mode.
    let repo = Repository::open_in(
        pool,
        replica_id,
        Some(master_secret),
        AccessMode::Read,
        false,
    )
    .await
    .unwrap();

    // Reading files is allowed.
    let mut file = repo.open_file("public.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"hello world");

    // Writing is not allowed.
    let local_branch = repo.local_branch().await.unwrap();
    file.seek(SeekFrom::Start(0)).await.unwrap();
    // short writes that don't cross block boundaries don't trigger the permission check which is
    // why the following works...
    file.write(b"hello universe", &local_branch).await.unwrap();
    // ...but flushing the file is not allowed.
    assert_matches!(file.flush().await, Err(Error::PermissionDenied));

    // Creating files works...
    let mut file = repo.create_file("hack.txt").await.unwrap();
    // ...but flushing it is not allowed.
    assert_matches!(file.flush().await, Err(Error::PermissionDenied));

    // Removing files is not allowed.
    assert_matches!(
        repo.remove_entry("public.txt").await,
        Err(Error::PermissionDenied)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_different_replica() {
    let pool = db::Pool::connect(":memory:").await.unwrap();
    let master_secret = MasterSecret::random();

    let replica_id_a = rand::random();
    let repo = Repository::create_in(
        pool.clone(),
        replica_id_a,
        master_secret.clone(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.local_branch().await.unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write(b"hello world", &local_branch).await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    let replica_id_b = rand::random();
    let repo = Repository::open_in(
        pool,
        replica_id_b,
        Some(master_secret),
        AccessMode::Read,
        false,
    )
    .await
    .unwrap();

    // The second replica doesn't have its own local branch in the repo.
    assert_matches!(
        repo.local_branch().await.map(|_| ()),
        Err(Error::PermissionDenied)
    );

    let mut file = repo.open_file("public.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"hello world");
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_remote_file() {
    let repo = Repository::create(
        &db::Store::Memory,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let remote_branch = repo
        .create_remote_branch(PublicKey::random())
        .await
        .unwrap();
    let remote_root = remote_branch.open_or_create_root().await.unwrap();

    let mut file = remote_root.create_file("test.txt".into()).await.unwrap();
    file.write(b"foo", &remote_branch).await.unwrap();
    file.flush().await.unwrap();

    let local_branch = repo.local_branch().await.unwrap();

    let mut file = repo.open_file("test.txt").await.unwrap();
    file.truncate(0, &local_branch).await.unwrap();
}

async fn read_file(repo: &Repository, path: impl AsRef<Utf8Path>) -> Vec<u8> {
    repo.open_file(path)
        .await
        .unwrap()
        .read_to_end()
        .await
        .unwrap()
}

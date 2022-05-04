use super::*;
use crate::{blob, block::BLOCK_SIZE, db};
use assert_matches::assert_matches;
use rand::Rng;
use std::io::SeekFrom;
use tokio::time::{sleep, timeout, Duration};

#[tokio::test(flavor = "multi_thread")]
async fn root_directory_always_exists() {
    let writer_id = rand::random();
    let repo = Repository::create(
        &db::Store::Temporary,
        writer_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();
    let _ = repo.open_directory("/").await.unwrap();
}

// Count leaf nodes in the index of the local branch.
async fn count_local_index_leaf_nodes(repo: &Repository) -> usize {
    let index = repo.index();
    let branch = repo.local_branch().await.unwrap();
    let mut conn = index.pool.acquire().await.unwrap();
    branch.data().count_leaf_nodes(&mut conn).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn count_leaf_nodes_sanity_checks() {
    let device_id = rand::random();

    let repo = Repository::create(
        &db::Store::Temporary,
        device_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

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

    repo.remove_entry(file_name).await.unwrap();

    // 1 = one for the root with a tombstone entry
    assert_eq!(count_local_index_leaf_nodes(&repo).await, 1);

    //------------------------------------------------------------------------
}

#[tokio::test(flavor = "multi_thread")]
async fn merge() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        true,
    )
    .await
    .unwrap();

    // Create remote branch and create a file in it.
    let remote_id = PublicKey::random();
    create_remote_file(&repo, remote_id, "test.txt", b"hello").await;

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
        &db::Store::Temporary,
        local_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

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
    let local_id = rand::random();
    let repo = Repository::create(
        &db::Store::Temporary,
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
        .flush()
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
        .flush()
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
        &db::Store::Temporary,
        writer_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let path = "/dir";
    let repo = Arc::new(repo);

    // The deadlock here happened because the reader lock when opening the directory is
    // acquired in the opposite order to the writer lock acqurired from flushing. I.e. the
    // reader lock acquires `/` and then `/dir`, but flushing acquires `/dir` first and
    // then `/`.
    let create_dir = scoped_task::spawn({
        let repo = repo.clone();
        async move {
            let dir = repo.create_directory(path).await.unwrap();
            dir.flush().await.unwrap();
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
                sleep(Duration::from_millis(10)).await;
            }
            panic!("Failed to open the directory after multiple attempts");
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
    let writer_id = rand::random();
    let repo = Repository::create(
        &db::Store::Temporary,
        writer_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

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
                        let actual_len = file.len().await;
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
async fn interleaved_flush() {
    let content1 = random_bytes(BLOCK_SIZE - blob::HEADER_SIZE); // 1 block
    let content2 = random_bytes(2 * BLOCK_SIZE - blob::HEADER_SIZE); // 2 blocks

    case(
        "write 2 blocks, write 0 blocks, flush 2nd then 1st",
        &content2,
        &[],
        FlushOrder::SecondThenFirst,
        3,
        &content2,
    )
    .await;

    case(
        "write 1 block, write 2 blocks, flush 1st then 2nd",
        &content1,
        &content2,
        FlushOrder::FirstThenSecond,
        3,
        &concat([nth_block(&content1, 0), nth_block(&content2, 1)]),
    )
    .await;

    case(
        "write 1 block, write 2 blocks, flush 2nd then 1st",
        &content1,
        &content2,
        FlushOrder::SecondThenFirst,
        3,
        &concat([nth_block(&content1, 0), nth_block(&content2, 1)]),
    )
    .await;

    enum FlushOrder {
        FirstThenSecond,
        SecondThenFirst,
    }

    // Open two instances of the same file, write `content0` to the first and `content1` to the
    // second, then flush in the specified order, finally check the total number of blocks in the
    // repository and the content of the file.
    async fn case(
        label: &str,
        content0: &[u8],
        content1: &[u8],
        flush_order: FlushOrder,
        expected_total_block_count: usize,
        expected_content: &[u8],
    ) {
        let repo = Repository::create(
            &db::Store::Temporary,
            rand::random(),
            MasterSecret::random(),
            AccessSecrets::random_write(),
            false,
        )
        .await
        .unwrap();
        let file_name = "test.txt";

        let mut file0 = repo.create_file(file_name).await.unwrap();
        file0.flush().await.unwrap();

        let mut file1 = repo.open_file(file_name).await.unwrap();

        file0.write(content0).await.unwrap();
        file1.write(content1).await.unwrap();

        match flush_order {
            FlushOrder::FirstThenSecond => {
                file0.flush().await.unwrap();
                file1.flush().await.unwrap();
            }
            FlushOrder::SecondThenFirst => {
                file1.flush().await.unwrap();
                file0.flush().await.unwrap();
            }
        }

        assert_eq!(
            count_local_index_leaf_nodes(&repo).await,
            expected_total_block_count,
            "'{}': unexpected total number of blocks in the repository",
            label
        );

        let actual_content = read_file(&repo, file_name).await;
        assert_eq!(
            actual_content.len(),
            expected_content.len(),
            "'{}': unexpected file content length",
            label
        );

        assert!(
            actual_content == expected_content,
            "'{}': unexpected file content",
            label
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn append_to_file() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let mut file = repo.create_file("foo.txt").await.unwrap();
    file.write(b"foo").await.unwrap();
    file.flush().await.unwrap();

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
    let pool = db::open_or_create(&db::Store::Temporary).await.unwrap();
    let device_id = rand::random();

    // Create the repo and put a file in it.
    let repo = Repository::create_in(
        pool.clone(),
        device_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
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
    for (master_secret, access_mode) in [
        (None, AccessMode::Blind),
        (Some(MasterSecret::random()), AccessMode::Write),
    ] {
        // Reopen the repo in blind mode.
        let repo = Repository::open_in(pool.clone(), device_id, master_secret, access_mode, false)
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

        // Reading the root directory is not allowed either.
        assert_matches!(repo.open_directory("/").await, Err(Error::PermissionDenied));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn blind_access_empty_repo() {
    let pool = db::open_or_create(&db::Store::Temporary).await.unwrap();
    let device_id = rand::random();

    // Create an empty repo.
    Repository::create_in(
        pool.clone(),
        device_id,
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    // Reopen the repo in blind mode.
    let repo = Repository::open_in(
        pool.clone(),
        device_id,
        Some(MasterSecret::random()),
        AccessMode::Read,
        false,
    )
    .await
    .unwrap();

    // Reading the root directory is not allowed.
    assert_matches!(repo.open_directory("/").await, Err(Error::PermissionDenied));
}

#[tokio::test(flavor = "multi_thread")]
async fn read_access_same_replica() {
    let pool = db::open_or_create(&db::Store::Temporary).await.unwrap();
    let device_id = rand::random();
    let master_secret = MasterSecret::random();

    let repo = Repository::create_in(
        pool.clone(),
        device_id,
        master_secret.clone(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write(b"hello world").await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    // Reopen the repo in read-only mode.
    let repo = Repository::open_in(
        pool,
        device_id,
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
    file.seek(SeekFrom::Start(0)).await.unwrap();
    // short writes that don't cross block boundaries don't trigger the permission check which is
    // why the following works...
    file.write(b"hello universe").await.unwrap();
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
    let pool = db::open_or_create(&db::Store::Temporary).await.unwrap();
    let master_secret = MasterSecret::random();

    let device_id_a = rand::random();
    let repo = Repository::create_in(
        pool.clone(),
        device_id_a,
        master_secret.clone(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let mut file = repo.create_file("public.txt").await.unwrap();
    file.write(b"hello world").await.unwrap();
    file.flush().await.unwrap();

    drop(file);
    drop(repo);

    let device_id_b = rand::random();
    let repo = Repository::open_in(
        pool,
        device_id_b,
        Some(master_secret),
        AccessMode::Read,
        false,
    )
    .await
    .unwrap();

    // The second replica doesn't have its own local branch in the repo.
    assert_matches!(repo.local_branch().await.map(|_| ()), None);

    let mut file = repo.open_file("public.txt").await.unwrap();
    let content = file.read_to_end().await.unwrap();
    assert_eq!(content, b"hello world");
}

#[tokio::test(flavor = "multi_thread")]
async fn truncate_forked_remote_file() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    create_remote_file(&repo, PublicKey::random(), "test.txt", b"foo").await;

    let local_branch = repo.get_or_create_local_branch().await.unwrap();
    let mut file = repo.open_file("test.txt").await.unwrap();
    file.fork(&local_branch).await.unwrap();
    file.truncate(0).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_modify_remote_file() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    create_remote_file(&repo, PublicKey::random(), "test.txt", b"foo").await;

    // HACK: during the above `create_remote_file` call the remote branch is opened in write mode
    // and then the remote root dir is cached. If the garbage collector kicks in at that time and
    // opens the remote root dir as well, it retains it in the cache even after
    // `create_remote_file` is finished. Then the following `open_file` might open the root from
    // the cache where it still has write mode and that would make the subsequent assertions to
    // fail because they expect it to have read-only mode. To prevent this we manually trigger
    // the garbage collector and wait for it to finish, to make sure the root dir is dropped and
    // removed from the cache. Then `open_file` reopens the root dir correctly in read-only mode.
    repo.collect_garbage().await.unwrap();

    let mut file = repo.open_file("test.txt").await.unwrap();
    assert_matches!(file.truncate(0).await, Err(Error::PermissionDenied));

    let mut file = repo.open_file("test.txt").await.unwrap();
    file.write(b"bar").await.unwrap();
    assert_matches!(file.flush().await, Err(Error::PermissionDenied));
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_create_file() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();
    let local_branch = repo.get_or_create_local_branch().await.unwrap();
    let local_id = *local_branch.id();

    // Initially the vvs are all empty
    let mut file = repo.create_file("parent/test.txt").await.unwrap();
    assert_eq!(file.version_vector().await, vv![]);
    assert_eq!(file.parent().version_vector().await, vv![]);
    assert_eq!(
        file.parent().parent().await.unwrap().version_vector().await,
        vv![]
    );

    file.flush().await.unwrap();
    // +1 for the file being created
    assert_eq!(file.version_vector().await, vv![local_id => 1]);
    // +1 for the parent being created
    // +1 for inserting the file
    assert_eq!(file.parent().version_vector().await, vv![local_id => 2]);
    // +1 for the root being created
    // +2 for inserting the parent
    assert_eq!(
        file.parent().parent().await.unwrap().version_vector().await,
        vv![local_id => 3]
    );

    file.write(b"blah").await.unwrap();
    file.flush().await.unwrap();
    // +1 for the file being modified
    assert_eq!(file.version_vector().await, vv![local_id => 2]);
    // +1 for the parent being modified due to file vv bump
    assert_eq!(file.parent().version_vector().await, vv![local_id => 3]);
    // +1 for the root being modified due to parent vv bump
    assert_eq!(
        file.parent().parent().await.unwrap().version_vector().await,
        vv![local_id => 4]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_deep_hierarchy() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();
    let local_branch = repo.get_or_create_local_branch().await.unwrap();
    let local_id = *local_branch.id();

    let depth = 10;
    let mut dirs = Vec::new();
    dirs.push(local_branch.open_or_create_root().await.unwrap());

    for i in 0..depth {
        let dir = dirs
            .last()
            .unwrap()
            .create_directory(format!("dir-{}", i))
            .await
            .unwrap();
        dirs.push(dir);
    }

    dirs.last().unwrap().flush().await.unwrap();

    // Each directory's local version is one less than its parent.
    for (index, dir) in dirs.iter().skip(1).enumerate() {
        assert_eq!(
            dir.version_vector().await,
            vv![local_id => (depth - index) as u64]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_recreate_deleted_file() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_id = *repo.get_or_create_local_branch().await.unwrap().id();

    let mut file = repo.create_file("test.txt").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    repo.remove_entry("test.txt").await.unwrap();

    let mut file = repo.create_file("test.txt").await.unwrap();
    file.flush().await.unwrap();

    assert_eq!(file.version_vector().await, vv![local_id => 3]);
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_fork_files() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.get_or_create_local_branch().await.unwrap();
    let remote_branch = repo
        .create_remote_branch(PublicKey::random())
        .await
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    let remote_root = remote_branch.open_or_create_root().await.unwrap();
    let remote_parent = remote_root.create_directory("parent".into()).await.unwrap();
    let mut file0 = create_file_in_directory(&remote_parent, "foo.txt", &[]).await;
    let mut file1 = create_file_in_directory(&remote_parent, "bar.txt", &[]).await;

    let remote_file0_vv = file0.version_vector().await;
    let remote_file1_vv = file1.version_vector().await;

    // Fork the first file
    file0.fork(&local_branch).await.unwrap();

    let local_parent = file0.parent();
    let local_root = local_parent.parent().await.unwrap();

    local_parent.flush().await.unwrap();

    // Parent and root are concurrent because the second file isn't forked yet.
    assert_eq!(file0.version_vector().await, remote_file0_vv);
    assert_eq!(
        local_parent
            .version_vector()
            .await
            .partial_cmp(&remote_parent.version_vector().await),
        None
    );
    assert_eq!(
        local_root
            .version_vector()
            .await
            .partial_cmp(&remote_root.version_vector().await),
        None
    );

    // Fork the second file
    file1.fork(&local_branch).await.unwrap();
    local_parent.flush().await.unwrap();

    // Parent and root are now newer.
    assert_eq!(file1.version_vector().await, remote_file1_vv);
    assert!(local_parent.version_vector().await > remote_parent.version_vector().await);
    assert!(local_root.version_vector().await > remote_root.version_vector().await);
}

#[tokio::test(flavor = "multi_thread")]
async fn version_vector_empty_directory() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_id = *repo.get_or_create_local_branch().await.unwrap().id();

    let dir = repo.create_directory("stuff").await.unwrap();
    assert_eq!(dir.version_vector().await, vv![]);

    dir.flush().await.unwrap();
    assert_eq!(dir.version_vector().await, vv![local_id => 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_conflict_modify_local() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.get_or_create_local_branch().await.unwrap();
    let local_id = *local_branch.id();

    let remote_id = PublicKey::random();
    let remote_branch = repo
        .create_remote_branch(remote_id)
        .await
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    // Create two concurrent versions of the same file.
    let local_file = create_file_in_branch(&local_branch, "test.txt", b"local v1").await;
    assert_eq!(local_file.version_vector().await, vv![local_id => 1]);
    drop(local_file);

    let remote_file = create_file_in_branch(&remote_branch, "test.txt", b"remote v1").await;
    assert_eq!(remote_file.version_vector().await, vv![remote_id => 1]);
    drop(remote_file);

    repo.collect_garbage().await.unwrap();

    // Modify the local version.
    let mut local_file = repo.open_file_version("test.txt", &local_id).await.unwrap();
    local_file.write(b"local v2").await.unwrap();
    local_file.flush().await.unwrap();
    drop(local_file);

    let mut local_file = repo.open_file_version("test.txt", &local_id).await.unwrap();
    assert_eq!(local_file.read_to_end().await.unwrap(), b"local v2");
    assert_eq!(local_file.version_vector().await, vv![local_id => 2]);

    let mut remote_file = repo
        .open_file_version("test.txt", &remote_id)
        .await
        .unwrap();
    assert_eq!(remote_file.read_to_end().await.unwrap(), b"remote v1");
    assert_eq!(remote_file.version_vector().await, vv![remote_id => 1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_conflict_attempt_to_fork_and_modify_remote() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.get_or_create_local_branch().await.unwrap();

    let remote_id = PublicKey::random();
    let remote_branch = repo
        .create_remote_branch(remote_id)
        .await
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
    remote_file.fork(&local_branch).await.unwrap();
    remote_file.write(b"remote v2").await.unwrap();
    assert_matches!(remote_file.flush().await, Err(Error::EntryExists));
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_branch() {
    let repo = Repository::create(
        &db::Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let local_branch = repo.get_or_create_local_branch().await.unwrap();

    let remote_id = PublicKey::random();
    let remote_branch = repo
        .create_remote_branch(remote_id)
        .await
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    // Keep the root dir open until we create both files to make sure it keeps write access.
    let remote_root = remote_branch.open_or_create_root().await.unwrap();
    create_file_in_directory(&remote_root, "foo.txt", b"foo").await;
    create_file_in_directory(&remote_root, "bar.txt", b"bar").await;
    drop(remote_root);

    let mut file = repo.open_file("foo.txt").await.unwrap();
    file.fork(&local_branch).await.unwrap();
    file.parent().flush().await.unwrap();
    drop(file);

    repo.shared.remove_branch(&remote_id).await.unwrap();

    // The forked file still exists
    assert_eq!(read_file(&repo, "foo.txt").await, b"foo");

    // The remote-only file is gone
    assert_matches!(repo.open_file("bar.txt").await, Err(Error::EntryNotFound));
}

async fn read_file(repo: &Repository, path: impl AsRef<Utf8Path>) -> Vec<u8> {
    repo.open_file(path)
        .await
        .unwrap()
        .read_to_end()
        .await
        .unwrap()
}

async fn create_remote_file(repo: &Repository, remote_id: PublicKey, name: &str, content: &[u8]) {
    let remote_branch = repo
        .create_remote_branch(remote_id)
        .await
        .unwrap()
        .reopen(repo.secrets().keys().unwrap());

    create_file_in_branch(&remote_branch, name, content).await;
}

async fn create_file_in_branch(branch: &Branch, name: &str, content: &[u8]) -> File {
    let root = branch.open_or_create_root().await.unwrap();
    create_file_in_directory(&root, name, content).await
}

async fn create_file_in_directory(dir: &Directory, name: &str, content: &[u8]) -> File {
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

fn nth_block(content: &[u8], index: usize) -> &[u8] {
    if index == 0 {
        &content[..BLOCK_SIZE - blob::HEADER_SIZE]
    } else {
        &content[BLOCK_SIZE - blob::HEADER_SIZE + (index - 1) * BLOCK_SIZE..][..BLOCK_SIZE]
    }
}

fn concat<const N: usize>(buffers: [&[u8]; N]) -> Vec<u8> {
    buffers.into_iter().fold(Vec::new(), |mut vec, buffer| {
        vec.extend_from_slice(buffer);
        vec
    })
}

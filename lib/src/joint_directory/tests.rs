use super::*;
use crate::{
    access_control::WriteSecrets, branch::Branch, crypto::sign::PublicKey, db, index::BranchData,
    sync::broadcast, version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::future;
use rand::{rngs::StdRng, SeedableRng};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn no_conflict() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();
    create_file(&mut conn, &root0, "file0.txt", &[], &branches[0]).await;

    let root1 = branches[1].open_or_create_root(&mut conn).await.unwrap();
    create_file(&mut conn, &root1, "file1.txt", &[], &branches[1]).await;

    let root = JointDirectory::new(Some(branches[0].clone()), [root0, root1]);
    let root = root.read().await;

    let entries: Vec<_> = root.entries().collect();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name(), "file0.txt");
    assert_eq!(entries[0].entry_type(), EntryType::File);
    assert_eq!(entries[1].name(), "file1.txt");
    assert_eq!(entries[1].entry_type(), EntryType::File);

    for (index, entry) in entries.iter().enumerate() {
        let name = format!("file{}.txt", index);

        let lookup: Vec<_> = root.lookup(&name).collect();
        assert_eq!(lookup.len(), 1);
        assert_eq!(lookup[0].name(), entry.name());

        let lookup_unique = root.lookup_unique(&name).unwrap();
        assert_eq!(lookup_unique.name(), entry.name());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_independent_files() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();
    create_file(&mut conn, &root0, "file.txt", &[], &branches[0]).await;

    let root1 = branches[1].open_or_create_root(&mut conn).await.unwrap();
    create_file(&mut conn, &root1, "file.txt", &[], &branches[1]).await;

    let root = JointDirectory::new(Some(branches[0].clone()), [root0, root1]);
    let root = root.read().await;

    let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();
    assert_eq!(files.len(), 2);

    for branch in &branches {
        let file = files
            .iter()
            .find(|file| file.branch().id() == branch.id())
            .unwrap();
        assert_eq!(file.name(), "file.txt");

        assert_matches!(
            root.lookup_unique(&versioned_file_name::create("file.txt", branch.id()))
                .unwrap(),
            JointEntryRef::File(file_ref) => {
                assert_eq!(file_ref.name(), file.name());
                assert!(file_ref.needs_disambiguation);
            }
        );
    }

    let files: Vec<_> = root
        .lookup("file.txt")
        .map(|entry| entry.file().unwrap())
        .collect();
    assert_eq!(files.len(), 2);

    for branch in &branches {
        let file = files
            .iter()
            .find(|file| file.branch().id() == branch.id())
            .unwrap();
        assert_eq!(file.name(), "file.txt");
    }

    assert_matches!(root.lookup_unique("file.txt"), Err(Error::AmbiguousEntry));

    assert_unique_and_ordered(2, root.entries());
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_forked_files() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();
    create_file(&mut conn, &root0, "file.txt", b"one", &branches[0]).await;

    // Fork the file into branch 1 and then modify it.
    let mut file1 = open_file(&mut conn, &root0, "file.txt").await;
    file1
        .fork_in_connection(&mut conn, &branches[1])
        .await
        .unwrap();
    file1.write_in_connection(&mut conn, b"two").await.unwrap();
    file1.flush_in_connection(&mut conn).await.unwrap();

    // Modify the file by branch 0 as well, to create concurrent versions
    let mut file0 = open_file(&mut conn, &root0, "file.txt").await;
    file0
        .write_in_connection(&mut conn, b"three")
        .await
        .unwrap();
    file0.flush_in_connection(&mut conn).await.unwrap();

    // Open branch 1's root dir which should have been created in the process.
    let root1 = branches[1].open_root(&mut conn).await.unwrap();

    let root = JointDirectory::new(Some(branches[1].clone()), [root0, root1]);
    let root = root.read().await;

    let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();

    assert_eq!(files.len(), 2);

    for branch in &branches {
        let file = files
            .iter()
            .find(|file| file.branch().id() == branch.id())
            .unwrap();
        assert_eq!(file.name(), "file.txt");
    }

    assert_unique_and_ordered(2, root.entries());
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_directories() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();
    root0
        .create_directory(&mut conn, "dir".to_owned())
        .await
        .unwrap();

    let root1 = branches[1].open_or_create_root(&mut conn).await.unwrap();
    root1
        .create_directory(&mut conn, "dir".to_owned())
        .await
        .unwrap();

    let root = JointDirectory::new(Some(branches[0].clone()), [root0, root1]);
    let root = root.read().await;

    let directories: Vec<_> = root
        .entries()
        .map(|entry| entry.directory().unwrap())
        .collect();
    assert_eq!(directories.len(), 1);
    assert_eq!(directories[0].name(), "dir");
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_file_and_directory() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();

    create_file(&mut conn, &root0, "config", &[], &branches[0]).await;

    let root1 = branches[1].open_or_create_root(&mut conn).await.unwrap();
    root1
        .create_directory(&mut conn, "config".to_owned())
        .await
        .unwrap();

    let root = JointDirectory::new(Some(branches[0].clone()), [root0, root1]);
    let root = root.read().await;

    let entries: Vec<_> = root.entries().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries.iter().map(|entry| entry.name()).collect::<Vec<_>>(),
        ["config", "config"]
    );
    assert!(entries.iter().any(|entry| match entry {
        JointEntryRef::File(file) => file.inner().branch().id() == branches[0].id(),
        JointEntryRef::Directory(_) => false,
    }));
    assert!(entries
        .iter()
        .any(|entry| entry.entry_type() == EntryType::Directory));

    let entries = root.lookup("config");
    assert_eq!(entries.count(), 2);

    let entry = root.lookup_unique("config").unwrap();
    assert_eq!(entry.entry_type(), EntryType::Directory);

    let name = versioned_file_name::create("config", branches[0].id());
    let entry = root.lookup_unique(&name).unwrap();
    assert_eq!(entry.entry_type(), EntryType::File);
    assert_eq!(entry.file().unwrap().branch().id(), branches[0].id());
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_identical_versions() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    // Create a file by one branch.
    let root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();
    create_file(&mut conn, &root0, "file.txt", b"one", &branches[0]).await;

    // Fork it into the other branch, creating an identical version of it.
    let mut file1 = open_file(&mut conn, &root0, "file.txt").await;
    file1
        .fork_in_connection(&mut conn, &branches[1])
        .await
        .unwrap();

    let root1 = branches[1].open_root(&mut conn).await.unwrap();

    // Create joint directory using branch 1 as the local branch.
    let root = JointDirectory::new(Some(branches[1].clone()), [root0, root1]);
    let root = root.read().await;

    // The file appears among the entries only once...
    assert_eq!(root.entries().count(), 1);

    // ...and it is the local version.
    let file = root
        .entries()
        .next()
        .unwrap()
        .file()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();
    assert_eq!(file.branch().id(), branches[1].id());

    // The file can also be retreived using `lookup`...
    let mut versions = root.lookup("file.txt");
    assert!(versions.next().is_some());
    assert!(versions.next().is_none());

    // ...and `lookup_version` using the author branch:
    root.lookup_version("file.txt", branches[0].id()).unwrap();
}

//// TODO: test conflict_forked_directories
//// TODO: test conflict_multiple_files_and_directories
//// TODO: test conflict_file_with_name_containing_branch_prefix

#[tokio::test(flavor = "multi_thread")]
async fn cd_into_concurrent_directory() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();

    let dir0 = root0
        .create_directory(&mut conn, "pics".to_owned())
        .await
        .unwrap();
    create_file(&mut conn, &dir0, "dog.jpg", &[], &branches[0]).await;

    let root1 = branches[1].open_or_create_root(&mut conn).await.unwrap();
    let dir1 = root1
        .create_directory(&mut conn, "pics".to_owned())
        .await
        .unwrap();
    create_file(&mut conn, &dir1, "cat.jpg", &[], &branches[1]).await;

    let root = JointDirectory::new(Some(branches[0].clone()), [root0, root1]);
    let dir = root.cd(&mut conn, "pics").await.unwrap();
    let dir = dir.read().await;

    let entries: Vec<_> = dir.entries().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name(), "cat.jpg");
    assert_eq!(entries[1].name(), "dog.jpg");
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_locally_non_existing_file() {
    // 0 - local, 1 - remote
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let content = b"cat";

    // Create local root dir
    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();

    // Create remote root dir
    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    // Create a file in the remote root
    create_file(&mut conn, &remote_root, "cat.jpg", content, &branches[1]).await;

    // Construct a joint directory over both root dirs and merge it.
    JointDirectory::new(Some(branches[0].clone()), [local_root.clone(), remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    // Verify the file now exists in the local branch.
    let local_content = open_file(&mut conn, &local_root, "cat.jpg")
        .await
        .read_to_end(&mut conn)
        .await
        .unwrap();
    assert_eq!(local_content, content);
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_locally_older_file() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let content_v0 = b"version 0";
    let content_v1 = b"version 1";

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    // Create a file in the remote root
    create_file(&mut conn, &remote_root, "cat.jpg", content_v0, &branches[1]).await;

    // Merge to transfer the file to the local branch
    JointDirectory::new(
        Some(branches[0].clone()),
        [local_root.clone(), remote_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    // Modify the file by the remote branch
    update_file(&mut conn, &remote_root, "cat.jpg", content_v1, &branches[1]).await;

    JointDirectory::new(Some(branches[0].clone()), [local_root.clone(), remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    let reader = local_root.read().await;
    let entry = reader.lookup("cat.jpg").unwrap().file().unwrap();

    let mut file = entry.open(&mut conn).await.unwrap();
    let local_content = file.read_to_end(&mut conn).await.unwrap();
    assert_eq!(local_content, content_v1);
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_locally_newer_file() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let content_v0 = b"version 0";
    let content_v1 = b"version 1";

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    create_file(&mut conn, &remote_root, "cat.jpg", content_v0, &branches[1]).await;

    JointDirectory::new(
        Some(branches[0].clone()),
        [local_root.clone(), remote_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    // Modify the file by the local branch
    update_file(&mut conn, &local_root, "cat.jpg", content_v1, &branches[0]).await;

    JointDirectory::new(Some(branches[0].clone()), [local_root.clone(), remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    let reader = local_root.read().await;

    let entry = reader.lookup("cat.jpg").unwrap().file().unwrap();

    let mut file = entry.open(&mut conn).await.unwrap();
    let local_content = file.read_to_end(&mut conn).await.unwrap();
    assert_eq!(local_content, content_v1);
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_merge_concurrent_file() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    create_file(&mut conn, &remote_root, "cat.jpg", b"v0", &branches[1]).await;

    JointDirectory::new(
        Some(branches[0].clone()),
        [local_root.clone(), remote_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    // Modify the file by both branches concurrently
    update_file(&mut conn, &local_root, "cat.jpg", b"v1", &branches[0]).await;
    update_file(&mut conn, &remote_root, "cat.jpg", b"v2", &branches[1]).await;

    // Merge succeeds but skips over the conflicting entries.
    JointDirectory::new(Some(branches[0].clone()), [local_root.clone(), remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    // The local version is unchanged
    assert_eq!(
        open_file(&mut conn, &local_root, "cat.jpg")
            .await
            .read_to_end(&mut conn)
            .await
            .unwrap(),
        b"v1"
    );

    // There is still only the local version in the local branch
    assert_eq!(local_root.read().await.entries().count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_merge_is_idempotent() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let vv0 = branches[0].version_vector(&mut conn).await.unwrap();

    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    // Merge after a remote modification - this causes local modification.
    create_file(&mut conn, &remote_root, "cat.jpg", b"v0", &branches[1]).await;

    JointDirectory::new(
        Some(branches[0].clone()),
        [local_root.clone(), remote_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    let vv1 = branches[0].version_vector(&mut conn).await.unwrap();
    assert!(vv1 > vv0);

    // Merge again. This time there is no local modification because there was no remote
    // modification either.
    JointDirectory::new(
        Some(branches[0].clone()),
        [local_root.clone(), remote_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    let vv2 = branches[0].version_vector(&mut conn).await.unwrap();
    assert_eq!(vv2, vv1);

    // Perform another remote modification and merge again - this causes local modification
    // again.
    update_file(&mut conn, &remote_root, "cat.jpg", b"v1", &branches[1]).await;

    JointDirectory::new(
        Some(branches[0].clone()),
        [local_root.clone(), remote_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    let vv3 = branches[0].version_vector(&mut conn).await.unwrap();
    assert!(vv3 > vv2);

    // Another idempotent merge which causes no local modification.
    JointDirectory::new(Some(branches[0].clone()), [local_root, remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    let vv4 = branches[0].version_vector(&mut conn).await.unwrap();
    assert_eq!(vv4, vv3);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_merge_is_idempotent() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    create_file(&mut conn, &remote_root, "cat.jpg", b"v0", &branches[1]).await;

    // First merge remote into local
    JointDirectory::new(
        Some(branches[0].clone()),
        [local_root.clone(), remote_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    let vv0 = branches[0].version_vector(&mut conn).await.unwrap();

    // Then merge local back into remote. This has no effect.
    JointDirectory::new(Some(branches[1].clone()), [remote_root, local_root])
        .merge(&mut conn)
        .await
        .unwrap();

    let vv1 = branches[0].version_vector(&mut conn).await.unwrap();
    assert_eq!(vv1, vv0);
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_remote_only() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    create_file(&mut conn, &remote_root, "cat.jpg", b"v0", &branches[1]).await;

    // When passing only the remote dir to the joint directory the merge still works.
    JointDirectory::new(Some(branches[0].clone()), [remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    let local_root = branches[0].open_root(&mut conn).await.unwrap();
    local_root.read().await.lookup("cat.jpg").unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_sequential_modifications() {
    let (pool, branches) = setup_with_rng(StdRng::seed_from_u64(0), 2).await;
    let mut conn = pool.acquire().await.unwrap();

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();

    // Create a file by local, then modify it by remote, then read it back by local verifying
    // the modification by remote got through.

    create_file(&mut conn, &local_root, "dog.jpg", b"v0", &branches[0]).await;

    let vv0 = read_version_vector(&local_root, "dog.jpg").await;

    JointDirectory::new(
        Some(branches[1].clone()),
        [remote_root.clone(), local_root.clone()],
    )
    .merge(&mut conn)
    .await
    .unwrap();

    let vv1 = read_version_vector(&remote_root, "dog.jpg").await;
    assert_eq!(vv1, vv0);

    update_file(&mut conn, &remote_root, "dog.jpg", b"v1", &branches[1]).await;

    let vv2 = read_version_vector(&remote_root, "dog.jpg").await;
    assert!(vv2 > vv1);

    JointDirectory::new(Some(branches[0].clone()), [local_root.clone(), remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    let reader = local_root.read().await;
    let entry = reader.lookup("dog.jpg").unwrap().file().unwrap();

    assert_eq!(entry.version_vector(), &vv2);

    let content = entry
        .open(&mut conn)
        .await
        .unwrap()
        .read_to_end(&mut conn)
        .await
        .unwrap();
    assert_eq!(content, b"v1");
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_concurrent_directories() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let local_dir = local_root
        .create_directory(&mut conn, "dir".into())
        .await
        .unwrap();
    create_file(&mut conn, &local_dir, "dog.jpg", &[], &branches[0]).await;

    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();
    let remote_dir = remote_root
        .create_directory(&mut conn, "dir".into())
        .await
        .unwrap();
    create_file(&mut conn, &remote_dir, "cat.jpg", &[], &branches[1]).await;

    JointDirectory::new(Some(branches[0].clone()), [local_root.clone(), remote_root])
        .merge(&mut conn)
        .await
        .unwrap();

    let local_root = local_root.read().await;

    assert_eq!(local_root.entries().count(), 1);

    let entry = local_root.entries().next().unwrap();
    assert_eq!(entry.name(), "dir");
    assert_matches!(entry, EntryRef::Directory(_));

    // +(2, 0) for creating local "dir" and "dog.jpg"
    // +(0, 2) for creating remote "dir" and "cat.jpg"
    // +(1, 0) for modifying local "dir" when the remote "cat.jpg" is merged into it
    let expected_vv = vv![*branches[0].id() => 3, *branches[1].id() => 2];
    assert_eq!(entry.version_vector(), &expected_vv);

    let dir = entry.directory().unwrap().open(&mut conn).await.unwrap();
    let dir = dir.read().await;

    assert_eq!(dir.entries().count(), 2);

    dir.lookup("dog.jpg").unwrap().file().unwrap();
    dir.lookup("cat.jpg").unwrap().file().unwrap();
}

// TODO: merge directory with missing blocks

#[tokio::test(flavor = "multi_thread")]
async fn remove_non_empty_subdirectory() {
    let (pool, branches) = setup(2).await;
    let mut conn = pool.acquire().await.unwrap();

    let local_root = branches[0].open_or_create_root(&mut conn).await.unwrap();
    let local_dir = local_root
        .create_directory(&mut conn, "dir0".into())
        .await
        .unwrap();
    create_file(&mut conn, &local_dir, "foo.txt", &[], &branches[0]).await;

    local_root
        .create_directory(&mut conn, "dir1".into())
        .await
        .unwrap();

    let remote_root = branches[1].open_or_create_root(&mut conn).await.unwrap();
    let remote_dir = remote_root
        .create_directory(&mut conn, "dir0".into())
        .await
        .unwrap();
    create_file(&mut conn, &remote_dir, "bar.txt", &[], &branches[1]).await;

    remote_root
        .create_directory(&mut conn, "dir2".into())
        .await
        .unwrap();

    let mut root =
        JointDirectory::new(Some(branches[0].clone()), [local_root.clone(), remote_root]);

    root.remove_entry_recursively(&mut conn, "dir0")
        .await
        .unwrap();

    let joint_reader = root.read().await;

    assert_matches!(joint_reader.lookup("dir0").next(), None);
    assert!(joint_reader.lookup("dir1").next().is_some());
    assert!(joint_reader.lookup("dir2").next().is_some());

    assert_matches!(
        local_root.read().await.lookup("dir0"),
        Ok(EntryRef::Tombstone(_))
    );

    assert_matches!(
        local_root.read().await.lookup("dir1"),
        Ok(EntryRef::Directory(_))
    );
}

async fn setup(branch_count: usize) -> (db::Pool, Vec<Branch>) {
    setup_with_rng(StdRng::from_entropy(), branch_count).await
}

// Useful for debugging non-deterministic failures.
async fn setup_with_rng(mut rng: StdRng, branch_count: usize) -> (db::Pool, Vec<Branch>) {
    let pool = db::create(&db::Store::Temporary).await.unwrap();
    let notify_tx = broadcast::Sender::new(1);
    let secrets = WriteSecrets::generate(&mut rng);

    let branches = {
        let pool = &pool;
        future::join_all((0..branch_count).map(|_| {
            let id = PublicKey::generate(&mut rng);
            let notify_tx = notify_tx.clone();
            let secrets = secrets.clone();

            async move {
                let data = BranchData::create(
                    &mut pool.acquire().await.unwrap(),
                    id,
                    &secrets.write_keys,
                    notify_tx,
                )
                .await
                .unwrap();
                Branch::new(pool.clone(), Arc::new(data), secrets.into())
            }
        }))
        .await
    };

    (pool, branches)
}

fn assert_unique_and_ordered<'a, I>(count: usize, mut entries: I)
where
    I: Iterator<Item = JointEntryRef<'a>>,
{
    let prev = entries.next();

    if prev.is_none() {
        assert!(count == 0);
        return;
    }

    let mut prev = prev.unwrap();
    let mut prev_i = 1;

    for entry in entries {
        assert!(prev.unique_name() < entry.unique_name());
        prev_i += 1;
        prev = entry;
    }

    assert_eq!(prev_i, count);
}

async fn create_file(
    conn: &mut db::Connection,
    parent: &Directory,
    name: &str,
    content: &[u8],
    local_branch: &Branch,
) -> File {
    let mut file = parent.create_file(conn, name.to_owned()).await.unwrap();

    if !content.is_empty() {
        file.fork_in_connection(conn, local_branch).await.unwrap();
        file.write_in_connection(conn, content).await.unwrap();
    }

    file.flush_in_connection(conn).await.unwrap();
    file
}

async fn update_file(
    conn: &mut db::Connection,
    parent: &Directory,
    name: &str,
    content: &[u8],
    local_branch: &Branch,
) {
    let mut file = open_file(conn, parent, name).await;

    file.fork_in_connection(conn, local_branch).await.unwrap();
    file.truncate(conn, 0).await.unwrap();
    file.write_in_connection(conn, content).await.unwrap();
    file.flush_in_connection(conn).await.unwrap();
}

async fn open_file(conn: &mut db::Connection, parent: &Directory, name: &str) -> File {
    parent
        .read()
        .await
        .lookup(name)
        .unwrap()
        .file()
        .unwrap()
        .open(conn)
        .await
        .unwrap()
}

async fn read_version_vector(parent: &Directory, name: &str) -> VersionVector {
    parent
        .read()
        .await
        .lookup(name)
        .unwrap()
        .file()
        .unwrap()
        .version_vector()
        .clone()
}

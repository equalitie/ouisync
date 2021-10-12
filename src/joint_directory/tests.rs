use super::*;
use crate::{
    blob::Blob, branch::Branch, crypto::Cryptor, db, directory::EntryData, index::BranchData,
    repository, version_vector::VersionVector,
};
use assert_matches::assert_matches;
use futures_util::future;
use rand::{distributions::Standard, rngs::StdRng, Rng, SeedableRng};
use std::{iter, sync::Arc};

#[tokio::test(flavor = "multi_thread")]
async fn no_conflict() {
    let branches = setup(2).await;

    let root0 = branches[0].open_or_create_root().await.unwrap();
    create_file(&root0, "file0.txt", &[]).await;

    let root1 = branches[1].open_or_create_root().await.unwrap();
    create_file(&root1, "file1.txt", &[]).await;

    let root = JointDirectory::new(vec![root0, root1]).await;
    let root = root.read().await;

    let entries: Vec<_> = root.entries().collect();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name(), "file0.txt");
    assert_eq!(entries[0].entry_type(), EntryType::File);
    assert_eq!(entries[1].name(), "file1.txt");
    assert_eq!(entries[1].entry_type(), EntryType::File);

    assert_eq!(root.lookup("file0.txt").collect::<Vec<_>>(), entries[0..1]);
    assert_eq!(root.lookup("file1.txt").collect::<Vec<_>>(), entries[1..2]);

    assert_eq!(root.lookup_unique("file0.txt").unwrap(), entries[0]);
    assert_eq!(root.lookup_unique("file1.txt").unwrap(), entries[1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_independent_files() {
    let branches = setup(2).await;

    let root0 = branches[0].open_or_create_root().await.unwrap();
    create_file(&root0, "file.txt", &[]).await;

    let root1 = branches[1].open_or_create_root().await.unwrap();
    create_file(&root1, "file.txt", &[]).await;

    let root = JointDirectory::new(vec![root0, root1]).await;
    let root = root.read().await;

    let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();
    assert_eq!(files.len(), 2);

    for branch in &branches {
        let file = files
            .iter()
            .find(|file| file.author() == branch.id())
            .unwrap();
        assert_eq!(file.name(), "file.txt");

        assert_eq!(
            root.lookup_unique(&versioned_file_name::create("file.txt", branch.id()))
                .unwrap(),
            JointEntryRef::File(JointFileRef {
                file: *file,
                needs_disambiguation: true
            })
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
            .find(|file| file.author() == branch.id())
            .unwrap();
        assert_eq!(file.name(), "file.txt");
    }

    assert_matches!(root.lookup_unique("file.txt"), Err(Error::AmbiguousEntry));

    assert_unique_and_ordered(2, root.entries());
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_forked_files() {
    let branches = setup(2).await;

    let root0 = branches[0].open_or_create_root().await.unwrap();
    create_file(&root0, "file.txt", b"one").await;

    // Open the file with branch 1 as the local branch and then modify it which copies (forks)
    // it into branch 1.
    let root0_by_1 = branches[0].open_root(branches[1].clone()).await.unwrap();
    let mut file1 = open_file_version(&root0_by_1, "file.txt", branches[0].id()).await;
    file1.write(b"two").await.unwrap();
    file1.flush().await.unwrap();

    // Modify the file by branch 0 as well, to create concurrent versions
    let mut file0 = open_file_version(&root0, "file.txt", branches[0].id()).await;
    file0.write(b"three").await.unwrap();
    file0.flush().await.unwrap();

    // Open branch 1's root dir which should have been created in the process.
    let root1 = branches[1].open_root(branches[1].clone()).await.unwrap();

    let root = JointDirectory::new(vec![root0_by_1, root1]).await;
    let root = root.read().await;

    let files: Vec<_> = root.entries().map(|entry| entry.file().unwrap()).collect();

    assert_eq!(files.len(), 2);

    for branch in &branches {
        let file = files
            .iter()
            .find(|file| file.author() == branch.id())
            .unwrap();
        assert_eq!(file.name(), "file.txt");
    }

    assert_unique_and_ordered(2, root.entries());
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_directories() {
    let branches = setup(2).await;

    let root0 = branches[0].open_or_create_root().await.unwrap();
    let dir0 = root0.create_directory("dir".to_owned()).await.unwrap();
    dir0.flush(None).await.unwrap();

    let root1 = branches[1].open_or_create_root().await.unwrap();
    let dir1 = root1.create_directory("dir".to_owned()).await.unwrap();
    dir1.flush(None).await.unwrap();

    let root = JointDirectory::new(vec![root0, root1]).await;
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
    let branches = setup(2).await;

    let root0 = branches[0].open_or_create_root().await.unwrap();

    create_file(&root0, "config", &[]).await;

    let root1 = branches[1].open_or_create_root().await.unwrap();

    let dir1 = root1.create_directory("config".to_owned()).await.unwrap();
    dir1.flush(None).await.unwrap();

    let root = JointDirectory::new(vec![root0, root1]).await;
    let root = root.read().await;

    let entries: Vec<_> = root.entries().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries.iter().map(|entry| entry.name()).collect::<Vec<_>>(),
        ["config", "config"]
    );
    assert!(entries.iter().any(|entry| match entry {
        JointEntryRef::File(file) => file.author() == branches[0].id(),
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
    assert_eq!(entry.file().unwrap().author(), branches[0].id());
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_identical_versions() {
    let branches = setup(2).await;

    // Create a file by one branch.
    let root0 = branches[0].open_or_create_root().await.unwrap();
    create_file(&root0, "file.txt", b"one").await;

    // Fork it into the other branch, creating an identical version of it.
    let root0_by_1 = branches[0].open_root(branches[1].clone()).await.unwrap();
    let mut file1 = open_file_version(&root0_by_1, "file.txt", branches[0].id()).await;
    file1.fork().await.unwrap();

    let root1 = branches[1].open_root(branches[1].clone()).await.unwrap();

    // Create joint directory using branch 1 as the local branch.
    let root = JointDirectory::new(vec![root0_by_1, root1]).await;
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
        .open()
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
    let branches = setup(2).await;

    let root0 = branches[0].open_or_create_root().await.unwrap();

    let dir0 = root0.create_directory("pics".to_owned()).await.unwrap();
    create_file(&dir0, "dog.jpg", &[]).await;

    let root1 = branches[1].open_or_create_root().await.unwrap();
    let dir1 = root1.create_directory("pics".to_owned()).await.unwrap();
    create_file(&dir1, "cat.jpg", &[]).await;

    let root = JointDirectory::new(vec![root0, root1]).await;
    let dir = root.cd("pics").await.unwrap();
    let dir = dir.read().await;

    let entries: Vec<_> = dir.entries().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name(), "cat.jpg");
    assert_eq!(entries[1].name(), "dog.jpg");
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_locally_non_existing_file() {
    // 0 - local, 1 - remote
    let branches = setup(2).await;

    let content = b"cat";

    // Create local root dir
    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();

    // Create remote root dir
    let remote_root = branches[1].open_or_create_root().await.unwrap();

    // Create a file in the remote root
    create_file(&remote_root, "cat.jpg", content).await;

    // Reopen the remote root locally.
    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    // Construct a joint directory over both root dirs and merge it.
    JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    // Verify the file now exists in the local branch.
    let local_content = open_file(&local_root, "cat.jpg")
        .await
        .read_to_end()
        .await
        .unwrap();
    assert_eq!(local_content, content);
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_locally_older_file() {
    let branches = setup(2).await;

    let content_v0 = b"version 0";
    let content_v1 = b"version 1";

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();

    let remote_root = branches[1].open_or_create_root().await.unwrap();

    // Create a file in the remote root
    create_file(&remote_root, "cat.jpg", content_v0).await;

    // Merge to transfer the file to the local branch
    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();
    let mut root =
        JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()]).await;
    root.merge().await.unwrap();

    // Modify the file by the remote branch
    update_file(&remote_root, "cat.jpg", content_v1).await;

    JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    let reader = local_root.read().await;

    // The remote version is newer, so it overwrites the local version and we end up with only
    // one version in the local branch.
    assert_eq!(reader.lookup("cat.jpg").unwrap().count(), 1);

    let entry = reader
        .lookup("cat.jpg")
        .unwrap()
        .next()
        .unwrap()
        .file()
        .unwrap();

    assert_eq!(entry.author(), branches[1].id());

    let mut file = entry.open().await.unwrap();
    let local_content = file.read_to_end().await.unwrap();
    assert_eq!(local_content, content_v1);
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_locally_newer_file() {
    let branches = setup(2).await;

    let content_v0 = b"version 0";
    let content_v1 = b"version 1";

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();

    let remote_root = branches[1].open_or_create_root().await.unwrap();

    create_file(&remote_root, "cat.jpg", content_v0).await;

    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();
    let mut root =
        JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()]).await;
    root.merge().await.unwrap();

    // Modify the file by the local branch
    update_file(&local_root, "cat.jpg", content_v1).await;

    JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    let reader = local_root.read().await;

    // The local version is newer, so there is only one version in the local branch.
    assert_eq!(reader.lookup("cat.jpg").unwrap().count(), 1);

    let entry = reader
        .lookup("cat.jpg")
        .unwrap()
        .next()
        .unwrap()
        .file()
        .unwrap();

    assert_eq!(entry.author(), branches[0].id());

    let mut file = entry.open().await.unwrap();
    let local_content = file.read_to_end().await.unwrap();
    assert_eq!(local_content, content_v1);
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_concurrent_file() {
    let branches = setup(2).await;

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();

    let remote_root = branches[1].open_or_create_root().await.unwrap();

    create_file(&remote_root, "cat.jpg", b"v0").await;

    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();
    let mut root =
        JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()]).await;
    root.merge().await.unwrap();

    // Modify the file by both branches concurrently
    update_file(&local_root, "cat.jpg", b"v1").await;
    update_file(&remote_root, "cat.jpg", b"v2").await;

    JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    // The versions are concurrent, so both are present in the local branch.
    assert_eq!(
        local_root.read().await.lookup("cat.jpg").unwrap().count(),
        2
    );

    assert_eq!(
        open_file_version(&local_root, "cat.jpg", branches[0].id())
            .await
            .read_to_end()
            .await
            .unwrap(),
        b"v1"
    );
    assert_eq!(
        open_file_version(&local_root, "cat.jpg", branches[1].id())
            .await
            .read_to_end()
            .await
            .unwrap(),
        b"v2"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn local_merge_is_idempotent() {
    let branches = setup(2).await;

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();

    let vv0 = branches[0].data().root_version_vector().await.clone();

    let remote_root = branches[1].open_or_create_root().await.unwrap();
    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    // Merge after a remote modification - this causes local modification.
    create_file(&remote_root, "cat.jpg", b"v0").await;
    JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
        .await
        .merge()
        .await
        .unwrap();

    let vv1 = branches[0].data().root_version_vector().await.clone();
    assert!(vv1 > vv0);

    // Merge again. This time there is no local modification because there was no remote
    // modification either.
    JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
        .await
        .merge()
        .await
        .unwrap();

    let vv2 = branches[0].data().root_version_vector().await.clone();
    assert_eq!(vv2, vv1);

    // Perform another remote modification and merge again - this causes local modification
    // again.
    update_file(&remote_root, "cat.jpg", b"v1").await;
    JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
        .await
        .merge()
        .await
        .unwrap();

    let vv3 = branches[0].data().root_version_vector().await.clone();
    assert!(vv3 > vv2);

    // Another idempotent merge which causes no local modification.
    JointDirectory::new(vec![local_root, remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    let vv4 = branches[0].data().root_version_vector().await.clone();
    assert_eq!(vv4, vv3);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_merge_is_idempotent() {
    let branches = setup(2).await;

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();
    let local_root_on_remote = branches[0].open_root(branches[1].clone()).await.unwrap();

    let remote_root = branches[1].open_or_create_root().await.unwrap();
    remote_root.flush(None).await.unwrap();
    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    create_file(&remote_root, "cat.jpg", b"v0").await;

    // First merge remote into local
    JointDirectory::new(vec![local_root, remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    let vv0 = branches[0].data().root_version_vector().await.clone();

    // Then merge local back into remote. This has no effect.
    JointDirectory::new(vec![remote_root, local_root_on_remote])
        .await
        .merge()
        .await
        .unwrap();

    let vv1 = branches[0].data().root_version_vector().await.clone();
    assert_eq!(vv1, vv0);
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_remote_only() {
    let branches = setup(2).await;

    let remote_root = branches[1].open_or_create_root().await.unwrap();
    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    create_file(&remote_root, "cat.jpg", b"v0").await;

    // When passing only the remote dir to the joint directory the merge still works.
    JointDirectory::new(iter::once(remote_root_on_local))
        .await
        .merge()
        .await
        .unwrap();

    let local_root = branches[0].open_root(branches[0].clone()).await.unwrap();
    local_root
        .read()
        .await
        .lookup_version("cat.jpg", branches[1].id())
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_sequential_modifications() {
    let branches = setup_with_rng(StdRng::seed_from_u64(0), 2).await;

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();
    let local_root_on_remote = branches[0].open_root(branches[1].clone()).await.unwrap();

    let remote_root = branches[1].open_or_create_root().await.unwrap();
    remote_root.flush(None).await.unwrap();
    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    // Create a file by local, then modify it by remote, then read it back by local verifying
    // the modification by remote got through.

    create_file(&local_root, "dog.jpg", b"v0").await;

    let vv0 = read_version_vector(&local_root, "dog.jpg").await;

    JointDirectory::new(vec![remote_root.clone(), local_root_on_remote])
        .await
        .merge()
        .await
        .unwrap();

    let vv1 = read_version_vector(&remote_root, "dog.jpg").await;
    assert_eq!(vv1, vv0);

    update_file(&remote_root, "dog.jpg", b"v1").await;

    let vv2 = read_version_vector(&remote_root, "dog.jpg").await;
    assert!(vv2 > vv1);

    JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    let reader = local_root.read().await;
    let entry = reader
        .lookup_version("dog.jpg", branches[1].id())
        .unwrap()
        .file()
        .unwrap();

    assert_eq!(entry.version_vector(), &vv2);

    let content = entry.open().await.unwrap().read_to_end().await.unwrap();
    assert_eq!(content, b"v1");
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_concurrent_directories() {
    let branches = setup(2).await;

    let local_root = branches[0].open_or_create_root().await.unwrap();
    let local_dir = local_root.create_directory("dir".into()).await.unwrap();
    create_file(&local_dir, "dog.jpg", &[]).await;

    let remote_root = branches[1].open_or_create_root().await.unwrap();
    let remote_dir = remote_root.create_directory("dir".into()).await.unwrap();
    create_file(&remote_dir, "cat.jpg", &[]).await;

    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    let local_root = local_root.read().await;

    assert_eq!(local_root.entries().count(), 1);

    let entry = local_root.entries().next().unwrap();
    assert_eq!(entry.name(), "dir");
    assert_matches!(entry, EntryRef::Directory(_));

    let expected_vv = {
        let mut vv = VersionVector::new();
        vv.insert(*branches[0].id(), 2); // 1: create, 2: add "dog.jpg"
        vv.insert(*branches[1].id(), 2); // 1: create, 2: add "cat.jpg"
        vv
    };
    assert_eq!(entry.version_vector(), &expected_vv);

    let dir = entry.directory().unwrap().open().await.unwrap();
    let dir = dir.read().await;

    assert_eq!(dir.entries().count(), 2);

    let entry = dir
        .lookup("dog.jpg")
        .unwrap()
        .next()
        .unwrap()
        .file()
        .unwrap();
    assert_eq!(entry.author(), branches[0].id());

    let entry = dir
        .lookup("cat.jpg")
        .unwrap()
        .next()
        .unwrap()
        .file()
        .unwrap();
    assert_eq!(entry.author(), branches[1].id());
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_missing_file() {
    let branches = setup(2).await;

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();

    let remote_root = branches[1].open_or_create_root().await.unwrap();
    create_dangling_file(&remote_root, "squirrel.jpg").await;

    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    // First attempt to merge fails because the file blob doesn't exist yet.
    match JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
        .await
        .merge()
        .await
    {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("unexpected success"),
    }

    assert_matches!(
        local_root
            .read()
            .await
            .lookup("squirrel.jpg")
            .map(|mut entries| entries.next()),
        Err(Error::EntryNotFound)
    );

    replace_dangling_file(&remote_root, "squirrel.jpg").await;

    // Merge again. This time it succeeds.
    JointDirectory::new(vec![local_root.clone(), remote_root_on_local])
        .await
        .merge()
        .await
        .unwrap();

    assert_matches!(
        local_root
            .read()
            .await
            .lookup("squirrel.jpg")
            .map(|mut entries| entries.next()),
        Ok(Some(_))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_missing_subdirectory() {
    let branches = setup(2).await;

    let local_root = branches[0].open_or_create_root().await.unwrap();
    local_root.flush(None).await.unwrap();

    let remote_root = branches[1].open_or_create_root().await.unwrap();
    create_dangling_directory(&remote_root, "animals").await;

    let remote_root_on_local = branches[1].open_root(branches[0].clone()).await.unwrap();

    // First attempt to merge fails because the subdirectory blob doesn't exist yet.
    match JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
        .await
        .merge()
        .await
    {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("unexpected success"),
    }

    assert_matches!(
        local_root
            .read()
            .await
            .lookup("animals")
            .map(|mut entries| entries.next()),
        Err(Error::EntryNotFound)
    );

    replace_dangling_directory(&remote_root, "animals").await;

    // Merge again. This time it succeeds.
    JointDirectory::new(vec![local_root.clone(), remote_root_on_local.clone()])
        .await
        .merge()
        .await
        .unwrap();

    assert_matches!(
        local_root
            .read()
            .await
            .lookup("animals")
            .map(|mut entries| entries.next()),
        Ok(Some(_))
    );
}

async fn setup(branch_count: usize) -> Vec<Branch> {
    setup_with_rng(StdRng::from_entropy(), branch_count).await
}

// Useful for debugging non-deterministic failures.
async fn setup_with_rng(rng: StdRng, branch_count: usize) -> Vec<Branch> {
    let pool = repository::open_db(&db::Store::Memory).await.unwrap();
    let pool = &pool;

    let ids = rng.sample_iter(Standard).take(branch_count);

    future::join_all(ids.map(|id| async move {
        let data = BranchData::new(pool, id).await.unwrap();
        Branch::new(pool.clone(), Arc::new(data), Cryptor::Null)
    }))
    .await
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

async fn create_file(parent: &Directory, name: &str, content: &[u8]) {
    let mut file = parent.create_file(name.to_owned()).await.unwrap();

    if !content.is_empty() {
        file.write(content).await.unwrap();
    }

    file.flush().await.unwrap();
}

async fn update_file(parent: &Directory, name: &str, content: &[u8]) {
    let mut file = open_file(parent, name).await;

    file.truncate(0).await.unwrap();
    file.write(content).await.unwrap();
    file.flush().await.unwrap();
}

async fn open_file(parent: &Directory, name: &str) -> File {
    parent
        .read()
        .await
        .lookup(name)
        .unwrap()
        .next()
        .unwrap()
        .file()
        .unwrap()
        .open()
        .await
        .unwrap()
}

async fn open_file_version(parent: &Directory, name: &str, branch_id: &ReplicaId) -> File {
    parent
        .read()
        .await
        .lookup_version(name, branch_id)
        .unwrap()
        .file()
        .unwrap()
        .open()
        .await
        .unwrap()
}

async fn read_version_vector(parent: &Directory, name: &str) -> VersionVector {
    parent
        .read()
        .await
        .lookup(name)
        .unwrap()
        .next()
        .unwrap()
        .file()
        .unwrap()
        .version_vector()
        .clone()
}

// TODO: try to reduce the code duplication in the following functions.

async fn create_dangling_file(parent: &Directory, name: &str) {
    create_dangling_entry(parent, EntryType::File, name).await
}

async fn create_dangling_directory(parent: &Directory, name: &str) {
    create_dangling_entry(parent, EntryType::Directory, name).await
}

async fn create_dangling_entry(parent: &Directory, entry_type: EntryType, name: &str) {
    let mut writer = parent.write().await;
    let branch_id = *writer.branch().id();
    let blob_id = rand::random();

    writer
        .insert_entry(
            name.into(),
            branch_id,
            EntryData::new(entry_type, blob_id, VersionVector::first(branch_id)),
            None,
        )
        .await
        .unwrap();
    writer.flush(None).await.unwrap();
}

async fn replace_dangling_file(parent: &Directory, name: &str) {
    let reader = parent.read().await;
    let old_blob_id = *reader
        .lookup_version(name, reader.branch().id())
        .unwrap()
        .file()
        .unwrap()
        .blob_id();

    // Create a dummy blob so the `create_file` call doesn't fail when trying to delete the
    // previous blob (which doesn't exists because the file was created as danling).
    Blob::create(reader.branch().clone(), Locator::Head(old_blob_id))
        .flush()
        .await
        .unwrap();

    drop(reader);

    parent
        .create_file(name.into())
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();
}

async fn replace_dangling_directory(parent: &Directory, name: &str) {
    let reader = parent.read().await;
    let old_blob_id = *reader
        .lookup_version(name, reader.branch().id())
        .unwrap()
        .directory()
        .unwrap()
        .blob_id();

    // Create a dummy blob so the `create_directory` call doesn't fail when trying to delete
    // the previous blob (which doesn't exists because the file was created as danling).
    Blob::create(reader.branch().clone(), Locator::Head(old_blob_id))
        .flush()
        .await
        .unwrap();

    drop(reader);

    parent
        .create_directory(name.into())
        .await
        .unwrap()
        .flush(None)
        .await
        .unwrap();
}

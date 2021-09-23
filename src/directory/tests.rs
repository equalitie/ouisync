use super::*;
use crate::{crypto::Cryptor, db, index::BranchData};
use assert_matches::assert_matches;
use futures_util::future;
use std::{array, collections::BTreeSet, convert::TryInto};

#[tokio::test(flavor = "multi_thread")]
async fn create_and_list_entries() {
    let branch = setup().await;

    // Create the root directory and put some file in it.
    let dir = branch.open_or_create_root().await.unwrap();

    let mut file_dog = dir.create_file("dog.txt".into()).await.unwrap();
    file_dog.write(b"woof").await.unwrap();
    file_dog.flush().await.unwrap();

    let mut file_cat = dir.create_file("cat.txt".into()).await.unwrap();
    file_cat.write(b"meow").await.unwrap();
    file_cat.flush().await.unwrap();

    dir.flush(None).await.unwrap();

    // Reopen the dir and try to read the files.
    let dir = branch.open_root(branch.clone()).await.unwrap();
    let dir = dir.read().await;

    let expected_names: BTreeSet<_> = vec!["dog.txt", "cat.txt"].into_iter().collect();
    let actual_names: BTreeSet<_> = dir.entries().map(|entry| entry.name()).collect();
    assert_eq!(actual_names, expected_names);

    for &(file_name, expected_content) in &[("dog.txt", b"woof"), ("cat.txt", b"meow")] {
        let mut versions: Vec<_> = dir.lookup(file_name).unwrap().collect();
        assert_eq!(versions.len(), 1);
        let mut file = versions
            .first_mut()
            .unwrap()
            .file()
            .unwrap()
            .open()
            .await
            .unwrap();
        let actual_content = file.read_to_end().await.unwrap();
        assert_eq!(actual_content, expected_content);
    }
}

// TODO: test update existing directory
#[tokio::test(flavor = "multi_thread")]
async fn add_entry_to_existing_directory() {
    let branch = setup().await;

    // Create empty directory
    let dir = branch.open_or_create_root().await.unwrap();
    dir.flush(None).await.unwrap();

    // Reopen it and add a file to it.
    let dir = branch.open_root(branch.clone()).await.unwrap();
    dir.create_file("none.txt".into()).await.unwrap();
    dir.flush(None).await.unwrap();

    // Reopen it again and check the file is still there.
    let dir = branch.open_root(branch.clone()).await.unwrap();
    assert!(dir.read().await.lookup("none.txt").is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_file() {
    let branch = setup().await;

    let name = "monkey.txt";

    // Create a directory with a single file.
    let parent_dir = branch.open_or_create_root().await.unwrap();
    let mut file = parent_dir.create_file(name.into()).await.unwrap();
    file.flush().await.unwrap();

    let file_vv = file.version_vector().await;
    let file_locator = *file.locator();

    // Reopen and remove the file
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();
    parent_dir
        .remove_file(name, branch.id(), file_vv)
        .await
        .unwrap();
    parent_dir.flush(None).await.unwrap();

    // Reopen again and check the file entry was removed.
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();
    let parent_dir = parent_dir.read().await;

    match parent_dir.lookup(name) {
        Err(Error::EntryNotFound) => panic!("expected to find a tombstone, but found nothing"),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(entries) => {
            let entries: Vec<_> = entries.collect();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].entry_type(), EntryType::Tombstone);
        }
    }

    assert_eq!(parent_dir.entries().count(), 1);

    // Check the file blob itself was removed as well.
    match Blob::open(branch.clone(), file_locator).await {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("file blob should not exists but it does"),
    }

    // Try re-creating the file again
    drop(parent_dir); // Drop the previous handle to avoid deadlock.
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();
    let mut file = parent_dir.create_file(name.into()).await.unwrap();
    file.flush().await.unwrap();
}

// Rename a file without moving it to another directory.
#[tokio::test(flavor = "multi_thread")]
async fn rename_file() {
    let branch = setup().await;

    let src_name = "zebra.txt";
    let dst_name = "donkey.txt";
    let content = b"hee-haw";

    // Create a directory with a single file.
    let parent_dir = branch.open_or_create_root().await.unwrap();
    let mut file = parent_dir.create_file(src_name.into()).await.unwrap();
    file.write(content).await.unwrap();
    file.flush().await.unwrap();

    let file_locator = *file.locator();

    // Reopen and move the file
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();

    parent_dir
        .move_entry(src_name, branch.id(), &parent_dir, dst_name)
        .await
        .unwrap();

    parent_dir.flush(None).await.unwrap();

    // Reopen again and check the file entry was removed.
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();
    let parent_dir = parent_dir.read().await;

    let mut dst_file = parent_dir
        .lookup_version(dst_name, branch.id())
        .unwrap()
        .file()
        .unwrap()
        .open()
        .await
        .unwrap();

    assert_eq!(&file_locator, dst_file.locator());
    assert_eq!(&content[..], &dst_file.read_to_end().await.unwrap()[..]);

    let src_entry = parent_dir.lookup_version(src_name, branch.id()).unwrap();

    assert_eq!(src_entry.entry_type(), EntryType::Tombstone);
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_subdirectory() {
    let branch = setup().await;

    let name = "dir";

    // Create a directory with a single subdirectory.
    let parent_dir = branch.open_or_create_root().await.unwrap();
    let dir = parent_dir.create_directory(name.into()).await.unwrap();
    dir.flush(None).await.unwrap();

    let dir_locator = *dir.read().await.locator();

    // Reopen and remove the subdirectory
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();
    parent_dir.remove_directory(name).await.unwrap();
    parent_dir.flush(None).await.unwrap();

    // Reopen again and check the subdirectory entry was removed.
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();
    let parent_dir = parent_dir.read().await;
    match parent_dir.lookup(name) {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("entry should not exists but it does"),
    }

    // Check the directory blob itself was removed as well.
    match Blob::open(branch, dir_locator).await {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("directory blob should not exists but it does"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fork() {
    let branches: [_; 2] = setup_multiple().await;

    // Create a nested directory by branch 0
    let root0 = branches[0].open_or_create_root().await.unwrap();
    root0.flush(None).await.unwrap();

    let dir0 = root0.create_directory("dir".into()).await.unwrap();
    dir0.flush(None).await.unwrap();

    // Fork it by branch 1 and modify it
    let dir0 = branches[0]
        .open_root(branches[1].clone())
        .await
        .unwrap()
        .read()
        .await
        .lookup_version("dir", branches[0].id())
        .unwrap()
        .directory()
        .unwrap()
        .open()
        .await
        .unwrap();
    let dir1 = dir0.fork().await.unwrap();

    dir1.create_file("dog.jpg".into()).await.unwrap();
    dir1.flush(None).await.unwrap();

    assert_eq!(dir1.read().await.branch().id(), branches[1].id());

    // Reopen orig dir and verify it's unchanged
    let dir = branches[0]
        .open_root(branches[0].clone())
        .await
        .unwrap()
        .read()
        .await
        .lookup_version("dir", branches[0].id())
        .unwrap()
        .directory()
        .unwrap()
        .open()
        .await
        .unwrap();

    assert_eq!(dir.read().await.entries().count(), 0);

    // Reopen forked dir and verify it contains the new file
    let dir = branches[1]
        .open_root(branches[1].clone())
        .await
        .unwrap()
        .read()
        .await
        .lookup_version("dir", branches[1].id())
        .unwrap()
        .directory()
        .unwrap()
        .open()
        .await
        .unwrap();

    assert_eq!(
        dir.read().await.entries().map(|entry| entry.name()).next(),
        Some("dog.jpg")
    );

    // Verify the root dir got forked as well
    branches[1].open_root(branches[1].clone()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn modify_directory_concurrently() {
    let branch = setup().await;
    let root = branch.open_or_create_root().await.unwrap();

    // Obtain two instances of the same directory, create a new file in one of them and verify
    // the file immediately exists in the other one as well.

    let dir0 = root.create_directory("dir".to_owned()).await.unwrap();
    let dir1 = root
        .read()
        .await
        .lookup("dir")
        .unwrap()
        .next()
        .unwrap()
        .directory()
        .unwrap()
        .open()
        .await
        .unwrap();

    let mut file0 = dir0.create_file("file.txt".to_owned()).await.unwrap();
    file0.write(b"hello").await.unwrap();
    file0.flush().await.unwrap();

    let mut file1 = dir1
        .read()
        .await
        .lookup("file.txt")
        .unwrap()
        .next()
        .unwrap()
        .file()
        .unwrap()
        .open()
        .await
        .unwrap();
    assert_eq!(file1.read_to_end().await.unwrap(), b"hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_entry_newer_than_existing() {
    let name = "foo.txt";
    let id0 = rand::random();
    let id1 = rand::random();

    // Test all permutations of the replica ids, to detect any unwanted dependency on their
    // order.
    for (a_author, b_author) in array::IntoIter::new([(id0, id1), (id1, id0)]) {
        let branch = setup().await;
        let root = branch.open_or_create_root().await.unwrap();

        let a_vv = VersionVector::first(a_author);

        let blob_id = root
            .insert_file_entry(name.to_owned(), a_author, a_vv.clone())
            .await
            .unwrap();

        // Need to create this dummy blob here otherwise the subsequent `insert_entry` would
        // fail when trying to delete it.
        Blob::create(branch.clone(), Locator::Head(blob_id))
            .flush()
            .await
            .unwrap();

        let b_vv = a_vv.increment(b_author);

        root.insert_file_entry(name.to_owned(), b_author, b_vv.clone())
            .await
            .unwrap();

        let reader = root.read().await;
        let mut entries = reader.entries();

        let entry = entries.next().unwrap().file().unwrap();
        assert_eq!(entry.author(), &b_author);
        assert_eq!(entry.version_vector(), &b_vv);

        assert!(entries.next().is_none());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_concurrent_file_version() {
    let name = "foo.txt";

    // Test both cases - removing the local version and removing the remote version.
    for index_to_remove in 0..2 {
        let branches: [_; 2] = setup_multiple().await;

        let local_id = branches[0].id();
        let remote_id = branches[1].id();

        let root = branches[0].open_or_create_root().await.unwrap();

        let mut vvs = Vec::new();

        // Create the concurrent versions
        for branch_id in branches.iter().map(|branch| *branch.id()) {
            let vv = VersionVector::first(branch_id);

            vvs.push(vv.clone());

            let blob_id = root
                .insert_file_entry(name.into(), branch_id, vv)
                .await
                .unwrap();

            Blob::create(branches[0].clone(), Locator::Head(blob_id))
                .flush()
                .await
                .unwrap();
        }

        let author_to_remove = branches[index_to_remove].id();
        let vv_to_remove = &vvs[index_to_remove];

        // Remove one of the versions
        {
            let mut writer = root.write().await;
            writer
                .remove_file(name, author_to_remove, vv_to_remove.clone())
                .await
                .unwrap();
            writer.flush(None).await.unwrap();
        }

        // Verify the removed version is gone but the other version remains
        let reader = root.read().await;

        if author_to_remove == local_id {
            // If we're removing a local version, then we replace the file with a tombstone.
            assert_matches!(
                reader.lookup_version(name, local_id),
                Ok(EntryRef::Tombstone(_))
            );
            assert_matches!(
                reader.lookup_version(name, remote_id),
                Ok(EntryRef::File(_))
            );
        } else {
            // If we're removing a remote version, then the VV of our version needs to be updated
            // to "happen after" the remote version being removed, and the remote version should
            // be removed together with its blob.
            assert_matches!(reader.lookup_version(name, local_id), Ok(EntryRef::File(_)));
            assert_matches!(
                reader.lookup_version(name, remote_id),
                Err(Error::EntryNotFound)
            );
        }
    }
}

async fn setup() -> Branch {
    let pool = db::init(db::Store::Memory).await.unwrap();
    create_branch(pool).await
}

async fn setup_multiple<const N: usize>() -> [Branch; N] {
    let pool = db::init(db::Store::Memory).await.unwrap();
    let branches: Vec<_> = future::join_all((0..N).map(|_| create_branch(pool.clone()))).await;
    branches.try_into().ok().unwrap()
}

async fn create_branch(pool: db::Pool) -> Branch {
    let branch_data = BranchData::new(&pool, rand::random()).await.unwrap();
    Branch::new(pool, Arc::new(branch_data), Cryptor::Null)
}

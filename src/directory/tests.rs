use super::*;
use crate::{crypto::Cryptor, db, index::BranchData};
use std::{array, collections::BTreeSet};

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

    let file_locator = *file.locator();

    // Reopen and remove the file
    let parent_dir = branch.open_root(branch.clone()).await.unwrap();
    parent_dir.remove_file(name, branch.id()).await.unwrap();
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
    let branch0 = setup().await;
    let branch1 = create_branch(branch0.db_pool().clone()).await;

    // Create a nested directory by branch 0
    let root0 = branch0.open_or_create_root().await.unwrap();
    root0.flush(None).await.unwrap();

    let dir0 = root0.create_directory("dir".into()).await.unwrap();
    dir0.flush(None).await.unwrap();

    // Fork it by branch 1 and modify it
    let dir0 = branch0
        .open_root(branch1.clone())
        .await
        .unwrap()
        .read()
        .await
        .lookup_version("dir", branch0.id())
        .unwrap()
        .directory()
        .unwrap()
        .open()
        .await
        .unwrap();
    let dir1 = dir0.fork().await.unwrap();

    dir1.create_file("dog.jpg".into()).await.unwrap();
    dir1.flush(None).await.unwrap();

    assert_eq!(dir1.read().await.branch().id(), branch1.id());

    // Reopen orig dir and verify it's unchanged
    let dir = branch0
        .open_root(branch0.clone())
        .await
        .unwrap()
        .read()
        .await
        .lookup_version("dir", branch0.id())
        .unwrap()
        .directory()
        .unwrap()
        .open()
        .await
        .unwrap();

    assert_eq!(dir.read().await.entries().count(), 0);

    // Reopen forked dir and verify it contains the new file
    let dir = branch1
        .open_root(branch1.clone())
        .await
        .unwrap()
        .read()
        .await
        .lookup_version("dir", branch1.id())
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
    branch1.open_root(branch1.clone()).await.unwrap();
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
            .insert_entry(
                name.to_owned(),
                a_author,
                EntryTypeWithBlob::File,
                a_vv.clone(),
            )
            .await
            .unwrap();

        // Need to create this dummy blob here otherwise the subsequent `insert_entry` would
        // fail when trying to delete it.
        Blob::create(branch.clone(), Locator::Head(blob_id))
            .flush()
            .await
            .unwrap();

        let b_vv = a_vv.increment(b_author);

        root.insert_entry(
            name.to_owned(),
            b_author,
            EntryTypeWithBlob::File,
            b_vv.clone(),
        )
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

async fn setup() -> Branch {
    let pool = db::init(db::Store::Memory).await.unwrap();
    create_branch(pool).await
}

async fn create_branch(pool: db::Pool) -> Branch {
    let branch_data = BranchData::new(&pool, rand::random()).await.unwrap();
    Branch::new(pool, Arc::new(branch_data), Cryptor::Null)
}

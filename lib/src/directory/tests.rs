use super::*;
use crate::{
    access_control::{AccessKeys, WriteSecrets},
    crypto::sign::Keypair,
    db,
    index::BranchData,
    sync::broadcast,
};
use assert_matches::assert_matches;
use futures_util::future;
use std::{collections::BTreeSet, convert::TryInto};

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

    dir.flush().await.unwrap();

    // Reopen the dir and try to read the files.
    let dir = branch.open_root().await.unwrap();
    let dir = dir.read().await;

    let expected_names: BTreeSet<_> = vec!["dog.txt", "cat.txt"].into_iter().collect();
    let actual_names: BTreeSet<_> = dir.entries().map(|entry| entry.name()).collect();
    assert_eq!(actual_names, expected_names);

    for &(file_name, expected_content) in &[("dog.txt", b"woof"), ("cat.txt", b"meow")] {
        let mut file = dir
            .lookup(file_name)
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
    dir.flush().await.unwrap();

    // Reopen it and add a file to it.
    let dir = branch.open_root().await.unwrap();
    dir.create_file("none.txt".into()).await.unwrap();
    dir.flush().await.unwrap();

    // Reopen it again and check the file is still there.
    let dir = branch.open_root().await.unwrap();
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
    let parent_dir = branch.open_root().await.unwrap();
    parent_dir
        .remove_entry(name, branch.id(), file_vv)
        .await
        .unwrap();
    parent_dir.flush().await.unwrap();

    // Reopen again and check the file entry was removed.
    let parent_dir = branch.open_root().await.unwrap();
    let parent_dir = parent_dir.read().await;

    assert_matches!(parent_dir.lookup(name), Ok(EntryRef::Tombstone(_)));
    assert_eq!(parent_dir.entries().count(), 1);

    // Check the file blob itself was removed as well.
    match Blob::open(branch.clone(), file_locator, Shared::uninit().into()).await {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("file blob should not exists but it does"),
    }

    // Try re-creating the file again
    drop(parent_dir); // Drop the previous handle to avoid deadlock.
    let parent_dir = branch.open_root().await.unwrap();
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
    let parent_dir = branch.open_root().await.unwrap();

    let entry_to_move = parent_dir
        .read()
        .await
        .lookup(src_name)
        .unwrap()
        .clone_data();

    parent_dir
        .move_entry(
            src_name,
            entry_to_move,
            &parent_dir,
            dst_name,
            VersionVector::first(*branch.id()),
        )
        .await
        .unwrap();

    parent_dir.flush().await.unwrap();

    // Reopen again and check the file entry was removed.
    let parent_dir = branch.open_root().await.unwrap();
    let parent_dir = parent_dir.read().await;

    let mut dst_file = parent_dir
        .lookup(dst_name)
        .unwrap()
        .file()
        .unwrap()
        .open()
        .await
        .unwrap();

    assert_eq!(&file_locator, dst_file.locator());
    assert_eq!(&content[..], &dst_file.read_to_end().await.unwrap()[..]);

    let src_entry = parent_dir.lookup(src_name).unwrap();

    assert_matches!(src_entry, EntryRef::Tombstone(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_within_branch() {
    let branch = setup().await;

    let file_name = "cow.txt";
    let content = b"moo";

    // Create a directory with a single file.
    let root_dir = branch.open_or_create_root().await.unwrap();
    let aux_dir = root_dir.create_directory("aux".into()).await.unwrap();

    let mut file = root_dir.create_file(file_name.into()).await.unwrap();
    file.write(content).await.unwrap();
    file.flush().await.unwrap();

    let file_locator = *file.locator();

    //
    // Move the file from ./ to ./aux/
    //

    let entry_to_move = root_dir
        .read()
        .await
        .lookup(file_name)
        .unwrap()
        .clone_data();

    root_dir
        .move_entry(
            file_name,
            entry_to_move,
            &aux_dir,
            file_name,
            VersionVector::first(*branch.id()),
        )
        .await
        .unwrap();

    let mut file = branch
        .open_root()
        .await
        .unwrap()
        .open_directory("aux")
        .await
        .unwrap()
        .open_file(file_name)
        .await
        .unwrap();

    assert_eq!(&file_locator, file.locator());
    assert_eq!(&content[..], &file.read_to_end().await.unwrap()[..]);

    //
    // Now move it back from ./aux/ to ./
    //

    let entry_to_move = aux_dir.read().await.lookup(file_name).unwrap().clone_data();

    let tombstone_vv = root_dir
        .read()
        .await
        .lookup(file_name)
        .unwrap()
        .version_vector()
        .clone();

    aux_dir
        .move_entry(
            file_name,
            entry_to_move,
            &root_dir,
            file_name,
            tombstone_vv.incremented(*branch.id()),
        )
        .await
        .unwrap();

    let mut file = root_dir.open_file(file_name).await.unwrap();

    assert_eq!(&content[..], &file.read_to_end().await.unwrap()[..]);
}

// Move directory "dir/" with content "cow.txt" to directory "dst/".
//
// Equivalent of:
//     $ mkdir dir
//     $ touch dir/cow.txt
//     $ mkdir dst
//     $ mv dir dst/
#[tokio::test(flavor = "multi_thread")]
async fn move_non_empty_directory() {
    let branch = setup().await;

    let dir_name = "dir";
    let dst_dir_name = "dst";
    let file_name = "cow.txt";
    let content = b"moo";

    // Create a directory with a single file.
    let root_dir = branch.open_or_create_root().await.unwrap();
    let dir = root_dir.create_directory(dir_name.into()).await.unwrap();

    let mut file = dir.create_file(file_name.into()).await.unwrap();
    file.write(content).await.unwrap();
    file.flush().await.unwrap();
    let file_locator = *file.locator();

    let dst_dir = root_dir
        .create_directory(dst_dir_name.into())
        .await
        .unwrap();

    let entry_to_move = root_dir.read().await.lookup(dir_name).unwrap().clone_data();

    root_dir
        .move_entry(
            dir_name,
            entry_to_move,
            &dst_dir,
            dir_name,
            VersionVector::first(*branch.id()),
        )
        .await
        .unwrap();

    let file = branch
        .open_root()
        .await
        .unwrap()
        .open_directory(dst_dir_name)
        .await
        .unwrap()
        .open_directory(dir_name)
        .await
        .unwrap()
        .open_file(file_name)
        .await
        .unwrap();

    assert_eq!(&file_locator, file.locator());
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_subdirectory() {
    let branch = setup().await;

    let name = "dir";

    // Create a directory with a single subdirectory.
    let parent_dir = branch.open_or_create_root().await.unwrap();
    let dir = parent_dir.create_directory(name.into()).await.unwrap();
    dir.flush().await.unwrap();

    let (dir_locator, dir_vv) = {
        let reader = dir.read().await;
        (*reader.locator(), reader.version_vector().await)
    };

    // Reopen and remove the subdirectory
    let parent_dir = branch.open_root().await.unwrap();
    parent_dir
        .remove_entry(name, branch.id(), dir_vv)
        .await
        .unwrap();
    parent_dir.flush().await.unwrap();

    // Reopen again and check the subdirectory entry was removed.
    let parent_dir = branch.open_root().await.unwrap();
    let parent_dir = parent_dir.read().await;
    assert_matches!(parent_dir.lookup(name), Ok(EntryRef::Tombstone(_)));

    // Check the directory blob itself was removed as well.
    match Blob::open(branch, dir_locator, Shared::uninit().into()).await {
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
    root0.flush().await.unwrap();

    let dir0 = root0.create_directory("dir".into()).await.unwrap();
    dir0.flush().await.unwrap();

    // Fork it by branch 1 and modify it
    let dir0 = branches[0]
        .open_root()
        .await
        .unwrap()
        .open_directory("dir")
        .await
        .unwrap();

    let dir1 = dir0.fork(&branches[1]).await.unwrap();

    dir1.create_file("dog.jpg".into()).await.unwrap();
    dir1.flush().await.unwrap();

    assert_eq!(dir1.read().await.branch().id(), branches[1].id());

    // Reopen orig dir and verify it's unchanged
    let dir = branches[0]
        .open_root()
        .await
        .unwrap()
        .open_directory("dir")
        .await
        .unwrap();

    assert_eq!(dir.read().await.entries().count(), 0);

    // Reopen forked dir and verify it contains the new file
    let dir = branches[1]
        .open_root()
        .await
        .unwrap()
        .open_directory("dir")
        .await
        .unwrap();

    assert_eq!(
        dir.read().await.entries().map(|entry| entry.name()).next(),
        Some("dog.jpg")
    );

    // Verify the root dir got forked as well
    branches[1].open_root().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn fork_over_tombstone() {
    let branches: [_; 2] = setup_multiple().await;

    // Create a directory in branch 0 and delete it.
    let root0 = branches[0].open_or_create_root().await.unwrap();
    root0
        .create_directory("dir".into())
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();
    let vv = root0
        .read()
        .await
        .lookup("dir")
        .unwrap()
        .version_vector()
        .clone();
    root0
        .remove_entry("dir", branches[0].id(), vv)
        .await
        .unwrap();

    // Create a directory with the same name in branch 1.
    let root1 = branches[1].open_or_create_root().await.unwrap();
    root1
        .create_directory("dir".into())
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    // Open it by branch 0 and fork it.
    let root1_on_0 = branches[1].open_root().await.unwrap();
    let dir1 = root1_on_0.open_directory("dir").await.unwrap();

    dir1.fork(&branches[0])
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    // Check the forked dir now exists in branch 0.
    assert_matches!(root0.read().await.lookup("dir"), Ok(EntryRef::Directory(_)));
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
    let id0 = PublicKey::random();
    let id1 = PublicKey::random();

    // Test all permutations of the replica ids, to detect any unwanted dependency on their
    // order.
    for (a_author, b_author) in [(id0, id1), (id1, id0)] {
        let branch = setup().await;
        let root = branch.open_or_create_root().await.unwrap();

        let a_vv = VersionVector::first(a_author);

        let blob_id = rand::random();
        root.inner
            .write()
            .await
            .insert_file_entry(name.to_owned(), a_vv.clone(), blob_id)
            .await
            .unwrap();

        // Need to create this dummy blob here otherwise the subsequent `insert_entry` would
        // fail when trying to delete it.
        Blob::create(branch.clone(), Locator::head(blob_id), Shared::uninit())
            .flush()
            .await
            .unwrap();

        let b_vv = a_vv.incremented(b_author);

        let blob_id = rand::random();
        root.inner
            .write()
            .await
            .insert_file_entry(name.to_owned(), b_vv.clone(), blob_id)
            .await
            .unwrap();

        let reader = root.read().await;
        let mut entries = reader.entries();

        let entry = entries.next().unwrap().file().unwrap();
        assert_eq!(entry.version_vector(), &b_vv);

        assert!(entries.next().is_none());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_unique_remote_file() {
    let local_branch = setup().await;
    let root = local_branch.open_or_create_root().await.unwrap();

    let name = "foo.txt";

    let remote_id = PublicKey::random();
    let remote_vv = vv![remote_id => 1];

    root.write()
        .await
        .remove_entry(
            name,
            &remote_id,
            remote_vv.clone(),
            OverwriteStrategy::Remove,
        )
        .await
        .unwrap();

    let local_vv = assert_matches!(
        root.read().await.lookup(name),
        Ok(EntryRef::Tombstone(entry)) => entry.version_vector().clone()
    );

    assert!(local_vv > remote_vv);
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_concurrent_remote_file() {
    let local_branch = setup().await;
    let root = local_branch.open_or_create_root().await.unwrap();

    let name = "foo.txt";
    root.create_file(name.to_owned())
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    let remote_id = PublicKey::random();
    let remote_vv = vv![remote_id => 1];

    root.write()
        .await
        .remove_entry(
            name,
            &remote_id,
            remote_vv.clone(),
            OverwriteStrategy::Remove,
        )
        .await
        .unwrap();

    let local_vv = assert_matches!(
        root.read().await.lookup(name),
        Ok(EntryRef::File(entry)) => entry.version_vector().clone()
    );

    assert!(local_vv > remote_vv);
}

async fn setup() -> Branch {
    let pool = db::create(&db::Store::Temporary).await.unwrap();
    let keys = WriteSecrets::random().into();
    create_branch(pool, keys).await
}

async fn setup_multiple<const N: usize>() -> [Branch; N] {
    let pool = db::create(&db::Store::Temporary).await.unwrap();
    let keys = AccessKeys::from(WriteSecrets::random());
    let branches: Vec<_> =
        future::join_all((0..N).map(|_| create_branch(pool.clone(), keys.clone()))).await;
    branches.try_into().ok().unwrap()
}

async fn create_branch(pool: db::Pool, keys: AccessKeys) -> Branch {
    let notify_tx = broadcast::Sender::new(1);
    let write_keys = Keypair::random();
    let branch_data = BranchData::create(
        &mut pool.acquire().await.unwrap(),
        PublicKey::random(),
        &write_keys,
        notify_tx,
    )
    .await
    .unwrap();
    Branch::new(pool, Arc::new(branch_data), keys)
}

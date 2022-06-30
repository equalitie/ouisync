use super::*;
use crate::{
    access_control::{AccessKeys, WriteSecrets},
    blob::{Blob, BlobCache},
    crypto::sign::Keypair,
    db,
    index::BranchData,
    sync::broadcast,
};
use assert_matches::assert_matches;
use futures_util::future;
use std::{collections::BTreeSet, convert::TryInto, sync::Arc};

#[tokio::test(flavor = "multi_thread")]
async fn create_and_list_entries() {
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    // Create the root directory and put some file in it.
    let mut dir = branch.open_or_create_root(&mut conn).await.unwrap();

    let mut file_dog = dir.create_file(&mut conn, "dog.txt".into()).await.unwrap();
    file_dog.write(&mut conn, b"woof").await.unwrap();
    file_dog.flush(&mut conn).await.unwrap();

    let mut file_cat = dir.create_file(&mut conn, "cat.txt".into()).await.unwrap();
    file_cat.write(&mut conn, b"meow").await.unwrap();
    file_cat.flush(&mut conn).await.unwrap();

    // Reopen the dir and try to read the files.
    let dir = branch.open_root(&mut conn).await.unwrap();
    let dir = dir.read().await;

    let expected_names: BTreeSet<_> = ["dog.txt", "cat.txt"].into_iter().collect();
    let actual_names: BTreeSet<_> = dir.entries().map(|entry| entry.name()).collect();
    assert_eq!(actual_names, expected_names);

    for &(file_name, expected_content) in &[("dog.txt", b"woof"), ("cat.txt", b"meow")] {
        let mut file = dir
            .lookup(file_name)
            .unwrap()
            .file()
            .unwrap()
            .open(&mut conn)
            .await
            .unwrap();
        let actual_content = file.read_to_end(&mut conn).await.unwrap();
        assert_eq!(actual_content, expected_content);
    }
}

// TODO: test update existing directory
#[tokio::test(flavor = "multi_thread")]
async fn add_entry_to_existing_directory() {
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    // Create empty directory and add a file to it.
    let mut dir = branch.open_or_create_root(&mut conn).await.unwrap();
    dir.create_file(&mut conn, "one.txt".into()).await.unwrap();

    // Reopen it and add another file to it.
    let mut dir = branch.open_root(&mut conn).await.unwrap();
    dir.create_file(&mut conn, "two.txt".into()).await.unwrap();

    // Reopen it again and check boths files are still there.
    let dir = branch.open_root(&mut conn).await.unwrap();
    let reader = dir.read().await;
    assert!(reader.lookup("one.txt").is_ok());
    assert!(reader.lookup("two.txt").is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_file() {
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let name = "monkey.txt";

    // Create a directory with a single file.
    let mut parent_dir = branch.open_or_create_root(&mut conn).await.unwrap();
    let mut file = parent_dir
        .create_file(&mut conn, name.into())
        .await
        .unwrap();
    file.flush(&mut conn).await.unwrap();

    let file_vv = file.version_vector(&mut conn).await.unwrap();
    let file_locator = *file.locator();
    drop(file);

    // Reopen and remove the file
    let parent_dir = branch.open_root(&mut conn).await.unwrap();
    parent_dir
        .remove_entry(&mut conn, name, branch.id(), file_vv)
        .await
        .unwrap();

    // Reopen again and check the file entry was removed.
    let parent_dir = branch.open_root(&mut conn).await.unwrap();
    let parent_dir = parent_dir.read().await;

    assert_matches!(parent_dir.lookup(name), Ok(EntryRef::Tombstone(_)));
    assert_eq!(parent_dir.entries().count(), 1);

    // Check the file blob itself was removed as well.
    match Blob::open(&mut conn, branch.clone(), file_locator, Shared::uninit()).await {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("file blob should not exists but it does"),
    }

    // Try re-creating the file again
    drop(parent_dir); // Drop the previous handle to avoid deadlock.
    let mut parent_dir = branch.open_root(&mut conn).await.unwrap();

    let mut file = parent_dir
        .create_file(&mut conn, name.into())
        .await
        .unwrap();
    file.flush(&mut conn).await.unwrap();
}

// Rename a file without moving it to another directory.
#[tokio::test(flavor = "multi_thread")]
async fn rename_file() {
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let src_name = "zebra.txt";
    let dst_name = "donkey.txt";
    let content = b"hee-haw";

    // Create a directory with a single file.
    let mut parent_dir = branch.open_or_create_root(&mut conn).await.unwrap();
    let mut file = parent_dir
        .create_file(&mut conn, src_name.into())
        .await
        .unwrap();
    file.write(&mut conn, content).await.unwrap();
    file.flush(&mut conn).await.unwrap();

    let file_locator = *file.locator();

    drop(file);

    // Reopen and move the file
    let parent_dir = branch.open_root(&mut conn).await.unwrap();

    let entry_to_move = parent_dir
        .read()
        .await
        .lookup(src_name)
        .unwrap()
        .clone_data();

    parent_dir
        .move_entry(
            &mut conn,
            src_name,
            entry_to_move,
            &parent_dir,
            dst_name,
            VersionVector::first(*branch.id()),
        )
        .await
        .unwrap();

    // Reopen again and check the file entry was removed.
    let parent_dir = branch.open_root(&mut conn).await.unwrap();
    let parent_dir = parent_dir.read().await;

    let mut dst_file = parent_dir
        .lookup(dst_name)
        .unwrap()
        .file()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    assert_eq!(&file_locator, dst_file.locator());
    assert_eq!(
        &content[..],
        &dst_file.read_to_end(&mut conn).await.unwrap()[..]
    );

    let src_entry = parent_dir.lookup(src_name).unwrap();

    assert_matches!(src_entry, EntryRef::Tombstone(_));
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_within_branch() {
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let file_name = "cow.txt";
    let content = b"moo";

    // Create a directory with a single file.
    let mut root_dir = branch.open_or_create_root(&mut conn).await.unwrap();
    let aux_dir = root_dir
        .create_directory(&mut conn, "aux".into())
        .await
        .unwrap();

    let mut file = root_dir
        .create_file(&mut conn, file_name.into())
        .await
        .unwrap();
    file.write(&mut conn, content).await.unwrap();
    file.flush(&mut conn).await.unwrap();
    root_dir.refresh(&mut conn).await.unwrap();

    let file_locator = *file.locator();

    drop(file);

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
            &mut conn,
            file_name,
            entry_to_move,
            &aux_dir,
            file_name,
            VersionVector::first(*branch.id()),
        )
        .await
        .unwrap();

    let mut file = branch
        .open_root(&mut conn)
        .await
        .unwrap()
        .read()
        .await
        .lookup("aux")
        .unwrap()
        .directory()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap()
        .read()
        .await
        .lookup(file_name)
        .unwrap()
        .file()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    assert_eq!(&file_locator, file.locator());
    assert_eq!(
        &content[..],
        &file.read_to_end(&mut conn).await.unwrap()[..]
    );

    drop(file);

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
            &mut conn,
            file_name,
            entry_to_move,
            &root_dir,
            file_name,
            tombstone_vv.incremented(*branch.id()),
        )
        .await
        .unwrap();

    let mut file = root_dir
        .read()
        .await
        .lookup(file_name)
        .unwrap()
        .file()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    assert_eq!(
        &content[..],
        &file.read_to_end(&mut conn).await.unwrap()[..]
    );
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
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let dir_name = "dir";
    let dst_dir_name = "dst";
    let file_name = "cow.txt";
    let content = b"moo";

    // Create a directory with a single file.
    let mut root_dir = branch.open_or_create_root(&mut conn).await.unwrap();
    let mut dir = root_dir
        .create_directory(&mut conn, dir_name.into())
        .await
        .unwrap();

    let mut file = dir.create_file(&mut conn, file_name.into()).await.unwrap();
    file.write(&mut conn, content).await.unwrap();
    file.flush(&mut conn).await.unwrap();

    dir.refresh(&mut conn).await.unwrap();
    root_dir.refresh(&mut conn).await.unwrap();

    let file_locator = *file.locator();

    let dst_dir = root_dir
        .create_directory(&mut conn, dst_dir_name.into())
        .await
        .unwrap();

    let entry_to_move = root_dir.read().await.lookup(dir_name).unwrap().clone_data();

    root_dir
        .move_entry(
            &mut conn,
            dir_name,
            entry_to_move,
            &dst_dir,
            dir_name,
            VersionVector::first(*branch.id()),
        )
        .await
        .unwrap();

    let file = branch
        .open_root(&mut conn)
        .await
        .unwrap()
        .read()
        .await
        .lookup(dst_dir_name)
        .unwrap()
        .directory()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap()
        .read()
        .await
        .lookup(dir_name)
        .unwrap()
        .directory()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap()
        .read()
        .await
        .lookup(file_name)
        .unwrap()
        .file()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    assert_eq!(&file_locator, file.locator());
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_subdirectory() {
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let name = "dir";

    // Create a directory with a single subdirectory.
    let mut parent_dir = branch.open_or_create_root(&mut conn).await.unwrap();
    let dir = parent_dir
        .create_directory(&mut conn, name.into())
        .await
        .unwrap();

    let (dir_locator, dir_vv) = {
        let reader = dir.read().await;
        (
            *reader.locator(),
            reader.version_vector(&mut conn).await.unwrap(),
        )
    };

    // Reopen and remove the subdirectory
    let parent_dir = branch.open_root(&mut conn).await.unwrap();
    parent_dir
        .remove_entry(&mut conn, name, branch.id(), dir_vv)
        .await
        .unwrap();

    // Reopen again and check the subdirectory entry was removed.
    let parent_dir = branch.open_root(&mut conn).await.unwrap();
    let parent_dir = parent_dir.read().await;
    assert_matches!(parent_dir.lookup(name), Ok(EntryRef::Tombstone(_)));

    // Check the directory blob itself was removed as well.
    match Blob::open(&mut conn, branch, dir_locator, Shared::uninit()).await {
        Err(Error::EntryNotFound) => (),
        Err(error) => panic!("unexpected error {:?}", error),
        Ok(_) => panic!("directory blob should not exists but it does"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fork() {
    let (pool, branches) = setup_multiple::<2>().await;
    let mut conn = pool.acquire().await.unwrap();

    // Create a nested directory by branch 0
    let mut root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();
    root0
        .create_directory(&mut conn, "dir".into())
        .await
        .unwrap();

    // Fork it by branch 1 and modify it
    let dir0 = {
        branches[0]
            .open_root(&mut conn)
            .await
            .unwrap()
            .read()
            .await
            .lookup("dir")
            .unwrap()
            .directory()
            .unwrap()
            .open(&mut conn)
            .await
            .unwrap()
    };

    let mut dir1 = dir0.fork(&mut conn, &branches[1]).await.unwrap();

    dir1.create_file(&mut conn, "dog.jpg".into()).await.unwrap();

    assert_eq!(dir1.read().await.branch().id(), branches[1].id());

    // Reopen orig dir and verify it's unchanged
    let dir = branches[0]
        .open_root(&mut conn)
        .await
        .unwrap()
        .read()
        .await
        .lookup("dir")
        .unwrap()
        .directory()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    assert_eq!(dir.read().await.entries().count(), 0);

    // Reopen forked dir and verify it contains the new file
    let dir = branches[1]
        .open_root(&mut conn)
        .await
        .unwrap()
        .read()
        .await
        .lookup("dir")
        .unwrap()
        .directory()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    assert_eq!(
        dir.read().await.entries().map(|entry| entry.name()).next(),
        Some("dog.jpg")
    );

    // Verify the root dir got forked as well
    branches[1].open_root(&mut conn).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn fork_over_tombstone() {
    let (pool, branches) = setup_multiple::<2>().await;
    let mut conn = pool.acquire().await.unwrap();

    // Create a directory in branch 0 and delete it.
    let mut root0 = branches[0].open_or_create_root(&mut conn).await.unwrap();
    root0
        .create_directory(&mut conn, "dir".into())
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
        .remove_entry(&mut conn, "dir", branches[0].id(), vv)
        .await
        .unwrap();

    // Create a directory with the same name in branch 1.
    let mut root1 = branches[1].open_or_create_root(&mut conn).await.unwrap();
    root1
        .create_directory(&mut conn, "dir".into())
        .await
        .unwrap();

    // Open it by branch 0 and fork it.
    let root1_on_0 = branches[1].open_root(&mut conn).await.unwrap();
    let dir1 = root1_on_0
        .read()
        .await
        .lookup("dir")
        .unwrap()
        .directory()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    dir1.fork(&mut conn, &branches[0]).await.unwrap();

    // Check the forked dir now exists in branch 0.
    root0.refresh(&mut conn).await.unwrap();
    assert_matches!(root0.read().await.lookup("dir"), Ok(EntryRef::Directory(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn modify_directory_concurrently() {
    let (pool, branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let mut root = branch.open_or_create_root(&mut conn).await.unwrap();

    // Obtain two instances of the same directory, create a new file in one of them and verify
    // the file also exists in the other after refresh.

    let mut dir0 = root
        .create_directory(&mut conn, "dir".to_owned())
        .await
        .unwrap();
    let dir1 = root
        .read()
        .await
        .lookup("dir")
        .unwrap()
        .directory()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();

    let mut file0 = dir0
        .create_file(&mut conn, "file.txt".to_owned())
        .await
        .unwrap();
    file0.write(&mut conn, b"hello").await.unwrap();
    file0.flush(&mut conn).await.unwrap();

    dir1.refresh(&mut conn).await.unwrap();
    let mut file1 = dir1
        .read()
        .await
        .lookup("file.txt")
        .unwrap()
        .file()
        .unwrap()
        .open(&mut conn)
        .await
        .unwrap();
    assert_eq!(file1.read_to_end(&mut conn).await.unwrap(), b"hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_unique_remote_file() {
    let (pool, local_branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let root = local_branch.open_or_create_root(&mut conn).await.unwrap();

    let name = "foo.txt";

    let remote_id = PublicKey::random();
    let remote_vv = vv![remote_id => 1];

    root.remove_entry(&mut conn, name, &remote_id, remote_vv.clone())
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
    let (pool, local_branch) = setup().await;
    let mut conn = pool.acquire().await.unwrap();

    let mut root = local_branch.open_or_create_root(&mut conn).await.unwrap();

    let name = "foo.txt";
    root.create_file(&mut conn, name.to_owned())
        .await
        .unwrap()
        .flush(&mut conn)
        .await
        .unwrap();

    let remote_id = PublicKey::random();
    let remote_vv = vv![remote_id => 1];

    root.remove_entry(&mut conn, name, &remote_id, remote_vv.clone())
        .await
        .unwrap();

    let local_vv = assert_matches!(
        root.read().await.lookup(name),
        Ok(EntryRef::File(entry)) => entry.version_vector().clone()
    );

    assert!(local_vv > remote_vv);
}

async fn setup() -> (db::Pool, Branch) {
    let pool = db::create(&db::Store::Temporary).await.unwrap();
    let keys = WriteSecrets::random().into();
    let branch = create_branch(&pool, keys).await;

    (pool, branch)
}

async fn setup_multiple<const N: usize>() -> (db::Pool, [Branch; N]) {
    let pool = db::create(&db::Store::Temporary).await.unwrap();
    let keys = AccessKeys::from(WriteSecrets::random());
    let branches: Vec<_> =
        future::join_all((0..N).map(|_| create_branch(&pool, keys.clone()))).await;
    let branches = branches.try_into().ok().unwrap();

    (pool, branches)
}

async fn create_branch(pool: &db::Pool, keys: AccessKeys) -> Branch {
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
    Branch::new(Arc::new(branch_data), keys, Arc::new(BlobCache::new()))
}

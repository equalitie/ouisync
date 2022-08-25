//! Garbage collection tests

mod common;

use self::common::{Env, Proto};
use ouisync::{db, File, Repository, BLOB_HEADER_SIZE, BLOCK_SIZE};
use rand::{rngs::StdRng, Rng};
use std::{io::SeekFrom, time::Duration};
use tokio::time;

#[tokio::test(flavor = "multi_thread")]
async fn local_delete_local_file() {
    let mut env = Env::with_seed(0);
    let repo = env.create_repo().await;

    assert_eq!(repo.count_blocks().await.unwrap(), 0);

    repo.create_file("test.dat").await.unwrap();

    // 1 block for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 2);

    repo.remove_entry("test.dat").await.unwrap();

    // just 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_delete_remote_file() {
    let mut env = Env::with_seed(0);

    let (network_l, network_r) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = network_l.handle().register(repo_l.store().clone());
    let _reg_r = network_r.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    let mut conn = repo_r.db().acquire().await.unwrap();
    write_to_file(
        &mut env.rng,
        &mut conn,
        &mut file,
        2 * BLOCK_SIZE - BLOB_HEADER_SIZE,
    )
    .await;
    file.flush(&mut conn).await.unwrap();

    // 2 blocks for the file + 1 block for the remote root directory
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 3))
        .await
        .unwrap();

    repo_l.remove_entry("test.dat").await.unwrap();
    repo_l.force_merge().await.unwrap();
    repo_l.force_garbage_collection().await.unwrap();

    // Both the file and the remote root are deleted, only the local root remains to track the
    // tombstone.
    assert_eq!(repo_l.count_blocks().await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_delete_remote_file() {
    let mut env = Env::with_seed(0);

    let (network_l, network_r) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = network_l.handle().register(repo_l.store().clone());
    let _reg_r = network_r.handle().register(repo_r.store().clone());

    repo_r.create_file("test.dat").await.unwrap();

    // 1 block for the file + 1 block for the remote root directory
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 2))
        .await
        .unwrap();

    repo_r.remove_entry("test.dat").await.unwrap();

    // The remote file is removed but the remote root remains to track the tombstone
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 1))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn local_truncate_local_file() {
    let mut env = Env::with_seed(0);
    let repo = env.create_repo().await;

    let mut file = repo.create_file("test.dat").await.unwrap();
    let mut conn = repo.db().acquire().await.unwrap();
    write_to_file(
        &mut env.rng,
        &mut conn,
        &mut file,
        2 * BLOCK_SIZE - BLOB_HEADER_SIZE,
    )
    .await;
    file.flush(&mut conn).await.unwrap();

    // 2 blocks for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 3);

    file.truncate(&mut conn, 0).await.unwrap();
    file.flush(&mut conn).await.unwrap();

    // 1 block for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_truncate_remote_file() {
    let mut env = Env::with_seed(0);

    let (network_l, network_r) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = network_l.handle().register(repo_l.store().clone());
    let _reg_r = network_r.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    let mut conn = repo_r.db().acquire().await.unwrap();
    write_to_file(
        &mut env.rng,
        &mut conn,
        &mut file,
        2 * BLOCK_SIZE - BLOB_HEADER_SIZE,
    )
    .await;
    file.flush(&mut conn).await.unwrap();

    // 2 blocks for the file + 1 block for the remote root directory
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 3))
        .await
        .unwrap();

    let mut file = repo_l.open_file("test.dat").await.unwrap();
    let mut conn = repo_l.db().acquire().await.unwrap();
    file.fork(
        &mut conn,
        repo_l.get_or_create_local_branch().await.unwrap(),
    )
    .await
    .unwrap();
    file.truncate(&mut conn, 0).await.unwrap();
    file.flush(&mut conn).await.unwrap();

    repo_l.force_merge().await.unwrap();
    repo_l.force_garbage_collection().await.unwrap();

    //   1 block for the file (the original 2 blocks were removed)
    // + 1 block for the local root (created when the file was forked)
    assert_eq!(repo_l.count_blocks().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_truncate_remote_file() {
    let mut env = Env::with_seed(0);

    let (network_l, network_r) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = network_l.handle().register(repo_l.store().clone());
    let _reg_r = network_r.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    let mut conn = repo_r.db().acquire().await.unwrap();
    write_to_file(
        &mut env.rng,
        &mut conn,
        &mut file,
        2 * BLOCK_SIZE - BLOB_HEADER_SIZE,
    )
    .await;
    file.flush(&mut conn).await.unwrap();

    // 2 blocks for the file + 1 block for the remote root
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 3))
        .await
        .unwrap();

    file.truncate(&mut conn, 0).await.unwrap();
    file.flush(&mut conn).await.unwrap();

    // 1 block for the file + 1 block for the remote root
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 2))
        .await
        .unwrap();
}

// FIXME: this currently fails because the first block of the file is not changed by the update and
// so is never redownloaded.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_delete_update() {
    let mut env = Env::with_seed(0);

    let (network_l, network_r) = common::create_connected_peers(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let reg_l = network_l.handle().register(repo_l.store().clone());
    let reg_r = network_r.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    let mut conn = repo_r.db().acquire().await.unwrap();
    write_to_file(
        &mut env.rng,
        &mut conn,
        &mut file,
        BLOCK_SIZE - BLOB_HEADER_SIZE,
    )
    .await;
    file.flush(&mut conn).await.unwrap();

    // 1 for the remote root + 1 for the file
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 2))
        .await
        .unwrap();

    // Disconnect to allow concurrent modifications.
    drop(reg_l);
    drop(reg_r);

    // Local delete
    repo_l.remove_entry("test.dat").await.unwrap();
    repo_l.force_garbage_collection().await.unwrap();

    // Sanity check
    assert_eq!(repo_l.count_blocks().await.unwrap(), 1);

    // Remote update. Don't change the length of the file so the first block (where the length it
    // stored) remains unchanged.
    file.seek(&mut conn, SeekFrom::End(-64)).await.unwrap();
    write_to_file(&mut env.rng, &mut conn, &mut file, 64).await;
    file.flush(&mut conn).await.unwrap();

    // Re-connect
    let _reg_l = network_l.handle().register(repo_l.store().clone());
    let _reg_r = network_r.handle().register(repo_r.store().clone());

    // 1 for the local root + 1 for the remote root + 2 for the file
    time::timeout(Duration::from_secs(5), expect_block_count(&repo_l, 4))
        .await
        .unwrap();
}

async fn expect_block_count(repo: &Repository, expected_count: usize) {
    common::eventually(repo, || async {
        let actual_count = repo.count_blocks().await.unwrap();
        if actual_count == expected_count {
            true
        } else {
            tracing::debug!(
                "block count - actual: {}, expected: {}",
                actual_count,
                expected_count
            );
            false
        }
    })
    .await
}

async fn write_to_file(rng: &mut StdRng, conn: &mut db::Connection, file: &mut File, size: usize) {
    let mut buffer = vec![0; size];
    rng.fill(&mut buffer[..]);
    file.write(conn, &buffer).await.unwrap();
}

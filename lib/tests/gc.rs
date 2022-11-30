//! Garbage collection tests

mod common;

use self::common::{Env, Proto};
use ouisync::{File, Repository, BLOB_HEADER_SIZE, BLOCK_SIZE};
use rand::{rngs::StdRng, Rng};

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

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.network.handle().register(repo_l.store().clone());
    let _reg_r = node_r.network.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    write_to_file(&mut env.rng, &mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
    file.flush().await.unwrap();

    // 2 blocks for the file + 1 block for the remote root directory
    expect_block_count(&repo_l, 3).await;

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

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.network.handle().register(repo_l.store().clone());
    let _reg_r = node_r.network.handle().register(repo_r.store().clone());

    repo_r.create_file("test.dat").await.unwrap();

    // 1 block for the file + 1 block for the remote root directory
    expect_block_count(&repo_l, 2).await;

    repo_r.remove_entry("test.dat").await.unwrap();

    // The remote file is removed but the remote root remains to track the tombstone
    expect_block_count(&repo_l, 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_truncate_local_file() {
    let mut env = Env::with_seed(0);
    let repo = env.create_repo().await;

    let mut file = repo.create_file("test.dat").await.unwrap();
    write_to_file(&mut env.rng, &mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
    file.flush().await.unwrap();

    // 2 blocks for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 3);

    file.truncate(0).await.unwrap();
    file.flush().await.unwrap();

    // 1 block for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_truncate_remote_file() {
    let mut env = Env::with_seed(0);

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.network.handle().register(repo_l.store().clone());
    let _reg_r = node_r.network.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    write_to_file(&mut env.rng, &mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
    file.flush().await.unwrap();

    // 2 blocks for the file + 1 block for the remote root directory
    expect_block_count(&repo_l, 3).await;

    let mut file = repo_l.open_file("test.dat").await.unwrap();
    file.fork(repo_l.local_branch().unwrap()).await.unwrap();
    file.truncate(0).await.unwrap();
    file.flush().await.unwrap();

    repo_l.force_merge().await.unwrap();
    repo_l.force_garbage_collection().await.unwrap();

    //   1 block for the file (the original 2 blocks were removed)
    // + 1 block for the local root (created when the file was forked)
    assert_eq!(repo_l.count_blocks().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_truncate_remote_file() {
    let mut env = Env::with_seed(0);

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.network.handle().register(repo_l.store().clone());
    let _reg_r = node_r.network.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    write_to_file(&mut env.rng, &mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
    file.flush().await.unwrap();

    // 2 blocks for the file + 1 block for the remote root
    expect_block_count(&repo_l, 3).await;

    file.truncate(0).await.unwrap();
    file.flush().await.unwrap();

    // 1 block for the file + 1 block for the remote root
    expect_block_count(&repo_l, 2).await;
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

async fn write_to_file(rng: &mut StdRng, file: &mut File, size: usize) {
    let mut buffer = vec![0; size];
    rng.fill(&mut buffer[..]);
    file.write(&buffer).await.unwrap();
}

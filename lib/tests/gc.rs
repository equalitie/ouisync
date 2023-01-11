//! Garbage collection tests

mod common;

use self::common::{old, Proto};
use ouisync::{File, Repository, BLOB_HEADER_SIZE, BLOCK_SIZE};
use rand::Rng;

#[tokio::test(flavor = "multi_thread")]
async fn local_delete_local_file() {
    let mut env = old::Env::new();
    let repo = env.create_repo().await;

    assert_eq!(repo.count_blocks().await.unwrap(), 0);

    repo.create_file("test.dat").await.unwrap();

    // 1 block for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 2);

    repo.remove_entry("test.dat").await.unwrap();
    repo.force_work().await.unwrap();

    // just 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_delete_remote_file() {
    let mut env = old::Env::new();

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.handle().register(repo_l.store().clone());
    let _reg_r = node_r.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    write_to_file(&mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
    file.flush().await.unwrap();

    // 2 blocks for the file + 1 block for the remote root directory
    expect_block_count(&repo_l, 3).await;

    repo_l.remove_entry("test.dat").await.unwrap();
    repo_l.force_work().await.unwrap();

    // Both the file and the remote root are deleted, only the local root remains to track the
    // tombstone.
    assert_eq!(repo_l.count_blocks().await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_delete_remote_file() {
    let mut env = old::Env::new();

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.handle().register(repo_l.store().clone());
    let _reg_r = node_r.handle().register(repo_r.store().clone());

    repo_r.create_file("test.dat").await.unwrap();

    // 1 block for the file + 1 block for the remote root directory
    expect_block_count(&repo_l, 2).await;

    repo_r.remove_entry("test.dat").await.unwrap();

    // The remote file is removed but the remote root remains to track the tombstone
    expect_block_count(&repo_l, 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_truncate_local_file() {
    let mut env = old::Env::new();
    let repo = env.create_repo().await;

    let mut file = repo.create_file("test.dat").await.unwrap();
    write_to_file(&mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
    file.flush().await.unwrap();

    repo.force_work().await.unwrap();

    // 2 blocks for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 3);

    file.truncate(0).await.unwrap();
    file.flush().await.unwrap();

    repo.force_work().await.unwrap();

    // 1 block for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn local_truncate_remote_file() {
    let mut env = old::Env::new();

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.handle().register(repo_l.store().clone());
    let _reg_r = node_r.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();
    write_to_file(&mut file, 2 * BLOCK_SIZE - BLOB_HEADER_SIZE).await;
    file.flush().await.unwrap();

    // 2 blocks for the file + 1 block for the remote root directory
    expect_block_count(&repo_l, 3).await;

    let mut file = repo_l.open_file("test.dat").await.unwrap();
    file.fork(repo_l.local_branch().unwrap()).await.unwrap();
    file.truncate(0).await.unwrap();
    file.flush().await.unwrap();

    repo_l.force_work().await.unwrap();

    //   1 block for the file (the original 2 blocks were removed)
    // + 1 block for the local root (created when the file was forked)
    assert_eq!(repo_l.count_blocks().await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_truncate_remote_file() {
    let mut env = old::Env::new();

    let (node_l, node_r) = env.create_connected_nodes(Proto::Tcp).await;
    let (repo_l, repo_r) = env.create_linked_repos().await;
    let _reg_l = node_l.handle().register(repo_l.store().clone());
    let _reg_r = node_r.handle().register(repo_r.store().clone());

    let mut file = repo_r.create_file("test.dat").await.unwrap();

    let mut content = vec![0; 2 * BLOCK_SIZE - BLOB_HEADER_SIZE];
    rand::thread_rng().fill(&mut content[..]);

    file.write(&content).await.unwrap();
    file.flush().await.unwrap();

    common::expect_file_content(&repo_l, "test.dat", &content).await;
    repo_l.force_work().await.unwrap();

    // 2 blocks for the file + 1 block for the remote root
    assert_eq!(repo_l.count_blocks().await.unwrap(), 3);

    file.truncate(0).await.unwrap();
    file.flush().await.unwrap();

    common::expect_file_content(&repo_l, "test.dat", &[]).await;
    repo_l.force_work().await.unwrap();

    // 1 block for the file + 1 block for the remote root
    assert_eq!(repo_l.count_blocks().await.unwrap(), 2);
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

async fn write_to_file(file: &mut File, size: usize) {
    let mut buffer = vec![0; size];
    rand::thread_rng().fill(&mut buffer[..]);
    file.write(&buffer).await.unwrap();
}

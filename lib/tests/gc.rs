//! Garbage collection tests

use ouisync::Repository;
use rand::{rngs::StdRng, SeedableRng};
use std::time::Duration;

mod common;

#[tokio::test(flavor = "multi_thread")]
async fn local_delete_local_file() {
    let mut rng = StdRng::seed_from_u64(0);
    let repo = common::create_repo(&mut rng).await;

    assert_eq!(repo.count_blocks().await.unwrap(), 0);

    repo.create_file("test.dat")
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    // 1 block for the file + 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 2);

    repo.remove_entry("test.dat").await.unwrap();

    // just 1 block for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 1);
}

// FIXME: currently when removing remote a file, we only put a tombstone into the local version of
// the parent directory but we don't delete the blocks of the file.
#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn local_delete_remote_file() {
    let mut rng = StdRng::seed_from_u64(0);

    let (network_a, network_b) = common::create_connected_peers().await;
    let (repo_a, repo_b) = common::create_linked_repos(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.index().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    repo_a
        .create_file("test.dat")
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    // 1 block for the file + 1 block for the remote root directory
    common::timeout(Duration::from_secs(5), expect_block_count(&repo_b, 2)).await;

    repo_b.remove_entry("test.dat").await.unwrap();

    // Both the remote file and the remote root directory are removed.
    assert_eq!(repo_b.count_blocks().await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_delete_remote_file() {
    let mut rng = StdRng::seed_from_u64(0);

    let (network_a, network_b) = common::create_connected_peers().await;
    let (repo_a, repo_b) = common::create_linked_repos(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.index().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    repo_a
        .create_file("test.dat")
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    // 1 block for the file + 1 block for the remote root directory
    common::timeout(Duration::from_secs(5), expect_block_count(&repo_b, 2)).await;

    repo_a.remove_entry("test.dat").await.unwrap();

    // The remote file is removed but the remote root remains to track the tombstone
    common::timeout(Duration::from_secs(5), expect_block_count(&repo_b, 1)).await;
}

async fn expect_block_count(repo: &Repository, expected_count: usize) {
    let mut rx = repo.subscribe();
    while rx.recv().await.is_ok() {
        if repo.count_blocks().await.unwrap() == expected_count {
            break;
        }
    }
}

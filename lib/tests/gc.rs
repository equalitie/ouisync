//! Garbage collection tests

use rand::{rngs::StdRng, SeedableRng};

mod common;

#[tokio::test(flavor = "multi_thread")]
async fn local_delete() {
    let mut rng = StdRng::seed_from_u64(0);
    let repo = common::create_repo(&mut rng).await;

    assert_eq!(repo.count_blocks().await.unwrap(), 0);

    let mut file = repo.create_file("test.dat").await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    // 1 for the root directory + 1 for the file
    assert_eq!(repo.count_blocks().await.unwrap(), 2);

    repo.remove_entry("test.dat").await.unwrap();

    // 1 for the root directory
    assert_eq!(repo.count_blocks().await.unwrap(), 1);
}

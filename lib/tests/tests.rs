use ouisync::{AccessSecrets, Error, MasterSecret, Network, NetworkOptions, Repository, Store};
use std::{net::Ipv4Addr, time::Duration};
use tokio::time;

#[tokio::test(flavor = "multi_thread")]
async fn relink_repository() {
    // Create two peers and connect them together.

    let network_a = Network::new::<String>(&test_network_options(), None)
        .await
        .unwrap();
    let network_b = Network::new::<String>(
        &NetworkOptions {
            peers: vec![*network_a.local_addr()],
            ..test_network_options()
        },
        None,
    )
    .await
    .unwrap();

    let repo_a = Repository::create(
        &Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        AccessSecrets::random_write(),
        false,
    )
    .await
    .unwrap();

    let repo_b = Repository::create(
        &Store::Temporary,
        rand::random(),
        MasterSecret::random(),
        repo_a.secrets().clone(),
        false,
    )
    .await
    .unwrap();

    let _reg_a = network_a.handle().register(repo_a.index().clone()).await;
    let reg_b = network_b.handle().register(repo_b.index().clone()).await;

    // Create a file by A
    let mut file_a = repo_a.create_file("test.txt").await.unwrap();
    file_a.write(b"first").await.unwrap();
    file_a.flush().await.unwrap();

    // Wait until the file is seen by B
    expect_file_content(&repo_b, "test.txt", b"first").await;

    // Unlink B's repo
    reg_b.cancel().await;

    // Update the file while B's repo is unlinked
    file_a.truncate(0).await.unwrap();
    file_a.write(b"second").await.unwrap();
    file_a.flush().await.unwrap();

    // Re-register B's repo
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    // Wait until the file is updated
    expect_file_content(&repo_b, "test.txt", b"second").await;
}

fn test_network_options() -> NetworkOptions {
    NetworkOptions {
        bind: Ipv4Addr::LOCALHOST.into(),
        disable_local_discovery: true,
        disable_upnp: true,
        disable_dht: true,
        ..Default::default()
    }
}

// Wait until the file at `path` has the expected content. Panics if timeout elapses before the
// file content matches.
async fn expect_file_content(repo: &Repository, path: &str, expected_content: &[u8]) {
    let mut rx = repo.subscribe();

    time::timeout(Duration::from_secs(5), async {
        while rx.recv().await.is_ok() {
            let mut file = match repo.open_file(path).await {
                Ok(file) => file,
                Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
                Err(error) => panic!("unexpected error: {:?}", error),
            };

            let actual_content = match file.read_to_end().await {
                Ok(content) => content,
                Err(Error::BlockNotFound(_)) => continue,
                Err(error) => panic!("unexpected error: {:?}", error),
            };

            if actual_content == expected_content {
                break;
            }
        }
    })
    .await
    .expect("timeout waiting for expected file content")
}

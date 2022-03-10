use ouisync::{
    AccessSecrets, ConfigStore, Error, MasterSecret, Network, NetworkOptions, Repository, Store,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::{net::Ipv4Addr, time::Duration};
use tokio::time;

#[tokio::test(flavor = "multi_thread")]
async fn relink_repository() {
    let mut rng = StdRng::seed_from_u64(0);

    // Create two peers and connect them together.
    let (network_a, network_b) = create_connected_peers().await;

    let repo_a = create_repo(&mut rng).await;
    let repo_b = create_repo_with_secrets(&mut rng, repo_a.secrets().clone()).await;

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

#[tokio::test(flavor = "multi_thread")]
async fn remove_remote_file() {
    let mut rng = StdRng::seed_from_u64(0);

    let (network_a, network_b) = create_connected_peers().await;

    let repo_a = create_repo(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.index().clone()).await;

    let repo_b = create_repo_with_secrets(&mut rng, repo_a.secrets().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    // Create a file by A and wait until B sees itb
    repo_a
        .create_file("test.txt")
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    expect_file_content(&repo_b, "test.txt", &[]).await;

    // Delete the file by B
    repo_b.remove_entry("test.txt").await.unwrap();

    // TODO: wait until A sees the file being deleted
}

// Create two `Network` instances connected together.
async fn create_connected_peers() -> (Network, Network) {
    let a = Network::new(&test_network_options(), ConfigStore::null())
        .await
        .unwrap();

    let b = Network::new(
        &NetworkOptions {
            peers: vec![*a.listener_local_addr()],
            ..test_network_options()
        },
        ConfigStore::null(),
    )
    .await
    .unwrap();

    (a, b)
}

async fn create_repo(rng: &mut StdRng) -> Repository {
    let secrets = AccessSecrets::generate_write(rng);
    create_repo_with_secrets(rng, secrets).await
}

async fn create_repo_with_secrets(rng: &mut StdRng, secrets: AccessSecrets) -> Repository {
    Repository::create(
        &Store::Temporary,
        rng.gen(),
        MasterSecret::generate(rng),
        secrets,
        false,
    )
    .await
    .unwrap()
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

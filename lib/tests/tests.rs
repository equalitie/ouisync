use ouisync::{
    AccessMode, AccessSecrets, ConfigStore, Error, File, MasterSecret, Network, NetworkOptions,
    Repository, Store,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::time;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread")]
async fn relink_repository() {
    env_logger::init();

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
    with_timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", b"first"),
    )
    .await;

    // Unlink B's repo
    reg_b.cancel().await;

    // Update the file while B's repo is unlinked
    file_a.truncate(0).await.unwrap();
    file_a.write(b"second").await.unwrap();
    file_a.flush().await.unwrap();

    // Re-register B's repo
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    // Wait until the file is updated
    with_timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", b"second"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_remote_file() {
    let mut rng = StdRng::seed_from_u64(0);

    let (network_a, network_b) = create_connected_peers().await;

    let repo_a = create_repo(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.index().clone()).await;

    let repo_b = create_repo_with_secrets(&mut rng, repo_a.secrets().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    // Create a file by A and wait until B sees it.
    repo_a
        .create_file("test.txt")
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    with_timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", &[]),
    )
    .await;

    // Delete the file by B
    repo_b.remove_entry("test.txt").await.unwrap();

    // TODO: wait until A sees the file being deleted
}

#[tokio::test(flavor = "multi_thread")]
async fn relay() {
    // Simulate two peers that can't connect to each other but both can connect to a third peer.
    // env_logger::init();

    // There used to be a deadlock that got triggered only when transferring a sufficiently large
    // file.
    let file_size = 4 * 1024 * 1024;

    let mut rng = StdRng::seed_from_u64(0);

    // The "relay" peer.
    let network_r = Network::new(&test_network_options(), ConfigStore::null())
        .await
        .unwrap();

    let network_a = create_peer_connected_to(*network_r.listener_local_addr()).await;
    let network_b = create_peer_connected_to(*network_r.listener_local_addr()).await;

    let repo_a = create_repo(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.index().clone()).await;

    let repo_b = create_repo_with_secrets(&mut rng, repo_a.secrets().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    let repo_r =
        create_repo_with_secrets(&mut rng, repo_a.secrets().with_mode(AccessMode::Blind)).await;
    let _reg_r = network_r.handle().register(repo_r.index().clone()).await;

    let mut content = vec![0; file_size];
    rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it. The file must pass through R because A and B
    // are not connected to each other.
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    // file.write(&content).await.unwrap();
    write_in_chunks(&mut file, &content, 4096).await;
    file.flush().await.unwrap();
    drop(file);

    with_timeout(
        Duration::from_secs(60 * 60),
        expect_file_content(&repo_b, "test.dat", &content),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_large_file() {
    env_logger::init();

    let file_size = 4 * 1024 * 1024;

    let mut rng = StdRng::seed_from_u64(0);

    let (network_a, network_b) = create_connected_peers().await;

    let repo_a = create_repo(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.index().clone()).await;

    let repo_b = create_repo_with_secrets(&mut rng, repo_a.secrets().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.index().clone()).await;

    let mut content = vec![0; file_size];
    rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it.
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    write_in_chunks(&mut file, &content, 4096).await;
    file.flush().await.unwrap();
    drop(file);

    with_timeout(
        Duration::from_secs(60 * 60),
        expect_file_content(&repo_b, "test.dat", &content),
    )
    .await;
}

// Create two `Network` instances connected together.
async fn create_connected_peers() -> (Network, Network) {
    let a = Network::new(&test_network_options(), ConfigStore::null())
        .await
        .unwrap();

    let b = create_peer_connected_to(*a.listener_local_addr()).await;

    (a, b)
}

// Create a `Network` instance connected only to the given address.
async fn create_peer_connected_to(addr: SocketAddr) -> Network {
    Network::new(
        &NetworkOptions {
            peers: vec![addr],
            ..test_network_options()
        },
        ConfigStore::null(),
    )
    .await
    .unwrap()
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
    while rx.recv().await.is_ok() {
        let mut file = match repo.open_file(path).await {
            Ok(file) => file,
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => continue,
            Err(error) => panic!("unexpected error: {:?}", error),
        };

        let actual_content = match read_in_chunks(&mut file, 4096).await {
            Ok(content) => content,
            Err(Error::BlockNotFound(_)) => continue,
            Err(error) => panic!("unexpected error: {:?}", error),
        };

        if actual_content == expected_content {
            break;
        }
    }
}

async fn with_timeout<F>(timeout: Duration, f: F) -> F::Output
where
    F: Future,
{
    time::timeout(timeout, f).await.expect("timeout expired")
}

async fn write_in_chunks(file: &mut File, content: &[u8], chunk_size: usize) {
    for offset in (0..content.len()).step_by(chunk_size) {
        let end = (offset + chunk_size).min(content.len());
        file.write(&content[offset..end]).await.unwrap();

        if to_megabytes(end) > to_megabytes(offset) {
            log::debug!(
                "file write progress: {}/{} MB",
                to_megabytes(end),
                to_megabytes(content.len())
            );
        }
    }
}

async fn read_in_chunks(file: &mut File, chunk_size: usize) -> Result<Vec<u8>, Error> {
    let mut content = vec![0; file.len().await as usize];
    let mut offset = 0;

    while offset < content.len() {
        let end = (offset + chunk_size).min(content.len());
        let size = file.read(&mut content[offset..end]).await?;
        offset += size;
    }

    Ok(content)
}

fn to_megabytes(bytes: usize) -> usize {
    bytes / 1024 / 1024
}

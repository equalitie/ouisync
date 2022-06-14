use ouisync::{AccessMode, ConfigStore, DbPool, Error, File, Network, Repository};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::time::Duration;
use tokio::time;

mod common;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread")]
async fn relink_repository() {
    // env_logger::init();

    let mut rng = StdRng::seed_from_u64(0);

    // Create two peers and connect them together.
    let (network_a, network_b) = common::create_connected_peers().await;

    let (repo_a, repo_b) = common::create_linked_repos(&mut rng).await;

    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let reg_b = network_b.handle().register(repo_b.store().clone());

    // Create a file by A
    let mut file_a = repo_a.create_file("test.txt").await.unwrap();
    file_a.write(b"first").await.unwrap();
    file_a.flush().await.unwrap();

    // Wait until the file is seen by B
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", b"first"),
    )
    .await
    .unwrap();

    // Unlink B's repo
    drop(reg_b);

    // Update the file while B's repo is unlinked
    file_a.truncate(0).await.unwrap();
    file_a.write(b"second").await.unwrap();
    file_a.flush().await.unwrap();

    // Re-register B's repo
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    // Wait until the file is updated
    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", b"second"),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_remote_file() {
    let mut rng = StdRng::seed_from_u64(0);

    let (network_a, network_b) = common::create_connected_peers().await;
    let (repo_a, repo_b) = common::create_linked_repos(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    // Create a file by A and wait until B sees it.
    repo_a
        .create_file("test.txt")
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();

    time::timeout(
        DEFAULT_TIMEOUT,
        expect_file_content(&repo_b, "test.txt", &[]),
    )
    .await
    .unwrap();

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
    let network_r = Network::new(&common::test_network_options(), ConfigStore::null())
        .await
        .unwrap();

    let network_a =
        common::create_peer_connected_to(*network_r.listener_local_addr_v4().unwrap()).await;
    let network_b =
        common::create_peer_connected_to(*network_r.listener_local_addr_v4().unwrap()).await;

    let repo_a = common::create_repo(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());

    let repo_b = common::create_repo_with_secrets(&mut rng, repo_a.secrets().clone()).await;
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let repo_r =
        common::create_repo_with_secrets(&mut rng, repo_a.secrets().with_mode(AccessMode::Blind))
            .await;
    let _reg_r = network_r.handle().register(repo_r.store().clone());

    let mut content = vec![0; file_size];
    rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it. The file must pass through R because A and B
    // are not connected to each other.
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    // file.write(&content).await.unwrap();
    write_in_chunks(&mut file, &content, 4096).await;
    file.flush().await.unwrap();
    drop(file);

    time::timeout(
        Duration::from_secs(60),
        expect_file_content(&repo_b, "test.dat", &content),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_large_file() {
    let file_size = 4 * 1024 * 1024;

    let mut rng = StdRng::seed_from_u64(0);

    let (network_a, network_b) = common::create_connected_peers().await;
    let (repo_a, repo_b) = common::create_linked_repos(&mut rng).await;
    let _reg_a = network_a.handle().register(repo_a.store().clone());
    let _reg_b = network_b.handle().register(repo_b.store().clone());

    let mut content = vec![0; file_size];
    rng.fill(&mut content[..]);

    // Create a file by A and wait until B sees it.
    let mut file = repo_a.create_file("test.dat").await.unwrap();
    write_in_chunks(&mut file, &content, 4096).await;
    file.flush().await.unwrap();
    drop(file);

    time::timeout(
        Duration::from_secs(60),
        expect_file_content(&repo_b, "test.dat", &content),
    )
    .await
    .unwrap();
}

// Wait until the file at `path` has the expected content. Panics if timeout elapses before the
// file content matches.
async fn expect_file_content(repo: &Repository, path: &str, expected_content: &[u8]) {
    let db = repo.db().clone();

    common::eventually(repo, || async {
        let mut file = match repo.open_file(path).await {
            Ok(file) => file,
            // `EntryNotFound` likely means that the parent directory hasn't yet been fully synced
            // and so the file entry is not in it yet.
            //
            // `BlockNotFound` means the first block of the file hasn't been downloaded yet.
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => return false,
            Err(error) => panic!("unexpected error: {:?}", error),
        };

        let actual_content = match read_in_chunks(&db, &mut file, 4096).await {
            Ok(content) => content,
            // `EntryNotFound` can still happen even here if merge runs in the middle of reading
            // the file - we opened the file while it was still in the remote branch but then that
            // branch got merged into the local one and deleted. That means the file no longer
            // exists in the remote branch and attempt to read from it further results in this
            // error.
            // TODO: this is not ideal as the only way to resolve this problem is to reopen the
            // file (unlike the `BlockNotFound` error where we just need to read it again when the
            // block gets downloaded). This should probably be considered a bug.
            //
            // `BlockNotFound` means just the some block of the file hasn't been downloaded yet.
            Err(Error::EntryNotFound | Error::BlockNotFound(_)) => return false,
            Err(error) => panic!("unexpected error: {:?}", error),
        };

        actual_content == expected_content
    })
    .await
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

async fn read_in_chunks(db: &DbPool, file: &mut File, chunk_size: usize) -> Result<Vec<u8>, Error> {
    let mut content = vec![0; file.len().await as usize];
    let mut offset = 0;

    while offset < content.len() {
        let mut conn = db.acquire().await?;

        let end = (offset + chunk_size).min(content.len());
        let size = file.read(&mut conn, &mut content[offset..end]).await?;
        offset += size;
    }

    Ok(content)
}

fn to_megabytes(bytes: usize) -> usize {
    bytes / 1024 / 1024
}

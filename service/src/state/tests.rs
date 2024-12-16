use std::{future, net::Ipv4Addr};

use crate::test_utils;

use super::*;
use assert_matches::assert_matches;
use futures_util::TryStreamExt;
use ouisync::{Access, AccessSecrets, Repository, RepositoryParams, WriteSecrets};
use tempfile::TempDir;
use tokio::time;
use tokio_stream::wrappers::ReadDirStream;
use tracing::Instrument;

#[tokio::test]
async fn normalize_repository_path() {
    let (temp_dir, mut state) = setup().await;
    let store_dir = temp_dir.path().join("store");
    state.set_store_dir(store_dir.clone()).await.unwrap();

    assert_eq!(
        state.normalize_repository_path(Path::new("foo")).unwrap(),
        store_dir.join("foo.ouisyncdb")
    );
    assert_eq!(
        state
            .normalize_repository_path(Path::new("foo/bar"))
            .unwrap(),
        store_dir.join("foo/bar.ouisyncdb")
    );
    assert_eq!(
        state
            .normalize_repository_path(Path::new("foo/bar.baz"))
            .unwrap(),
        store_dir.join("foo/bar.baz.ouisyncdb")
    );
    assert_eq!(
        state
            .normalize_repository_path(Path::new("foo.ouisyncdb"))
            .unwrap(),
        store_dir.join("foo.ouisyncdb")
    );
    assert_eq!(
        state
            .normalize_repository_path(Path::new("/home/alice/repos/foo"))
            .unwrap(),
        Path::new("/home/alice/repos/foo.ouisyncdb")
    );
}

#[tokio::test]
async fn non_unique_repository_name() {
    let (_temp_dir, mut state) = setup().await;
    let path = Path::new("test");

    assert_matches!(
        state
            .create_repository(path, None, None, None, false, false, false)
            .await,
        Ok(_)
    );

    assert_matches!(
        state
            .create_repository(path, None, None, None, false, false, false)
            .await,
        Err(Error::RepositoryExists)
    );
}

#[tokio::test]
async fn non_unique_repository_id() {
    let (_temp_dir, mut state) = setup().await;

    let token = ShareToken::from(AccessSecrets::Write(WriteSecrets::random()));

    assert_matches!(
        state
            .create_repository(
                Path::new("foo"),
                None,
                None,
                Some(token.clone()),
                false,
                false,
                false
            )
            .await,
        Ok(_)
    );

    // different name but same token
    assert_matches!(
        state
            .create_repository(
                Path::new("bar"),
                None,
                None,
                Some(token),
                false,
                false,
                false
            )
            .await,
        Err(Error::RepositoryExists)
    );
}

// TODO: test import repo with non-unique id

#[tokio::test]
async fn open_already_opened_repository() {
    let (_temp_dir, mut state) = setup().await;

    let name = "foo";
    let handle0 = state
        .create_repository(Path::new(name), None, None, None, false, false, false)
        .await
        .unwrap();

    // Open by full path
    let handle1 = state
        .open_repository(
            &state
                .store_dir()
                .unwrap()
                .join(name)
                .with_extension(REPOSITORY_FILE_EXTENSION),
            None,
        )
        .await
        .unwrap();
    assert_eq!(handle1, handle0);

    // Open by partial path
    let handle2 = state.open_repository(Path::new(name), None).await.unwrap();
    assert_eq!(handle2, handle0);
}

// TODO: test reopen with higher access mode

#[tokio::test]
async fn expire_empty_repository() {
    test_utils::init_log();

    let (_temp_dir, mut state) = setup().await;

    let secrets = WriteSecrets::random();

    let name = "foo";
    let handle = state
        .create_repository(
            Path::new(name),
            None,
            None,
            Some(ShareToken::from(AccessSecrets::Blind { id: secrets.id })),
            false,
            false,
            false,
        )
        .await
        .unwrap();

    // Repository expiration requires block expiration to be enabled as well.
    state
        .set_block_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();

    state
        .set_repository_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();

    time::sleep(Duration::from_secs(1)).await;

    state.delete_expired_repositories().await;

    assert_eq!(state.find_repository(name), Err(FindError::NotFound));
    assert_eq!(
        read_dir(state.store_dir().unwrap()).await,
        Vec::<PathBuf>::new()
    );
}

#[tokio::test]
async fn expire_synced_repository() {
    test_utils::init_log();

    let temp_dir = TempDir::new().unwrap();

    let secrets = WriteSecrets::random();

    let (remote_network, _remote_repo, _remote_reg) = async {
        let monitor = StateMonitor::make_root();

        let repo = Repository::create(
            &RepositoryParams::new(temp_dir.path().join("remote/repo.ouisyncdb"))
                .with_monitor(monitor.make_child("repo")),
            Access::WriteUnlocked {
                secrets: secrets.clone(),
            },
        )
        .await
        .unwrap();

        let mut file = repo.create_file("test.txt").await.unwrap();
        file.write_all(b"hello world").await.unwrap();
        file.flush().await.unwrap();
        drop(file);

        let network = Network::new(monitor, None, None);
        network
            .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;

        let reg = network.register(repo.handle());

        (network, repo, reg)
    }
    .instrument(tracing::info_span!("remote"))
    .await;

    let remote_addr = remote_network
        .listener_local_addrs()
        .into_iter()
        .next()
        .unwrap();

    let mut local_state = State::init(temp_dir.path().join("local/config"))
        .instrument(tracing::info_span!("local"))
        .await
        .unwrap();

    local_state
        .set_store_dir(temp_dir.path().join("local/store"))
        .await
        .unwrap();
    local_state
        .bind_network(vec![PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
        .await;
    local_state.network.add_user_provided_peer(&remote_addr);

    let name = "foo";
    let handle = local_state
        .create_repository(
            Path::new(name),
            None,
            None,
            Some(ShareToken::from(AccessSecrets::Blind { id: secrets.id })),
            true,
            false,
            false,
        )
        .await
        .unwrap();
    let local_repo = local_state.repos.get(handle).unwrap().repository();

    // Wait until synced
    let mut rx = local_repo.subscribe();

    time::timeout(Duration::from_secs(30), async {
        loop {
            let progress = local_repo.sync_progress().await.unwrap();

            if progress.total > 0 && progress.value == progress.total {
                break;
            }

            rx.recv().await.unwrap();
        }
    })
    .await
    .unwrap();

    // Enable expiration
    local_state
        .set_block_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();
    local_state
        .set_repository_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();

    time::sleep(Duration::from_secs(1)).await;

    local_state.delete_expired_repositories().await;

    assert_eq!(local_state.find_repository(name), Err(FindError::NotFound));
    assert_eq!(
        read_dir(local_state.store_dir().unwrap()).await,
        Vec::<PathBuf>::new(),
    );
}

#[tokio::test]
async fn move_repository() {
    let (_temp_dir, mut state) = setup().await;
    let src = Path::new("foo");
    let dst = Path::new("bar");

    let repo = state
        .create_repository(src, None, None, None, false, false, false)
        .await
        .unwrap();
    let src_full = state.repository_path(repo).unwrap();

    let file = state.create_file(repo, "test.txt".into()).await.unwrap();
    state.write_file(file, 0, b"hello").await.unwrap();
    state.close_file(file).await.unwrap();

    state.move_repository(repo, dst).await.unwrap();
    let dst_full = state.repository_path(repo).unwrap();

    let file = state.open_file(repo, "test.txt").await.unwrap();
    let len = state.file_len(file).unwrap();
    let content = state.read_file(file, 0, len).await.unwrap();
    assert_eq!(content, b"hello");

    assert!(fs::try_exists(dst_full).await.unwrap());

    // Check none of the src files (including the aux files) exist anymore
    let src_files: Vec<_> =
        ReadDirStream::new(fs::read_dir(src_full.parent().unwrap()).await.unwrap())
            .try_filter(|entry| {
                future::ready(
                    entry
                        .path()
                        .to_str()
                        .unwrap()
                        .starts_with(src_full.to_str().unwrap()),
                )
            })
            .map_ok(|entry| entry.path())
            .try_collect()
            .await
            .unwrap();

    assert_eq!(src_files, Vec::<PathBuf>::new());
}

async fn setup() -> (TempDir, State) {
    let temp_dir = TempDir::new().unwrap();
    let mut state = State::init(temp_dir.path().join("config")).await.unwrap();
    state
        .set_store_dir(temp_dir.path().join("store"))
        .await
        .unwrap();

    (temp_dir, state)
}

async fn read_dir(path: impl AsRef<Path>) -> Vec<PathBuf> {
    ReadDirStream::new(fs::read_dir(path).await.unwrap())
        .map_ok(|entry| entry.path())
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
}

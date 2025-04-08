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
    let store_dir = PathBuf::from("/home/alice/ouisync");
    let store = Store::new(Some(store_dir.clone()));

    assert_eq!(
        store.normalize_repository_path(Path::new("foo")).unwrap(),
        store_dir.join("foo.ouisyncdb")
    );
    assert_eq!(
        store
            .normalize_repository_path(Path::new("foo/bar"))
            .unwrap(),
        store_dir.join("foo/bar.ouisyncdb")
    );
    assert_eq!(
        store
            .normalize_repository_path(Path::new("foo/bar.baz"))
            .unwrap(),
        store_dir.join("foo/bar.baz.ouisyncdb")
    );
    assert_eq!(
        store
            .normalize_repository_path(Path::new("foo.ouisyncdb"))
            .unwrap(),
        store_dir.join("foo.ouisyncdb")
    );
    assert_eq!(
        store
            .normalize_repository_path(Path::new("/home/alice/repos/foo"))
            .unwrap(),
        Path::new("/home/alice/repos/foo.ouisyncdb")
    );
}

#[tokio::test]
async fn non_unique_repository_name() {
    let (_temp_dir, state) = setup().await;
    let path = Path::new("test");

    assert_matches!(
        state
            .session_create_repository(path.into(), None, None, None, false, false, false)
            .await,
        Ok(_)
    );

    assert_matches!(
        state
            .session_create_repository(path.into(), None, None, None, false, false, false)
            .await,
        Err(Error::AlreadyExists)
    );
}

#[tokio::test]
async fn non_unique_repository_id() {
    let (_temp_dir, state) = setup().await;

    let token = ShareToken::from(AccessSecrets::Write(WriteSecrets::random()));

    assert_matches!(
        state
            .session_create_repository(
                PathBuf::from("foo"),
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
            .session_create_repository(
                PathBuf::from("bar"),
                None,
                None,
                Some(token),
                false,
                false,
                false
            )
            .await,
        Err(Error::AlreadyExists)
    );
}

// TODO: test import repo with non-unique id

#[tokio::test]
async fn open_already_opened_repository() {
    let (_temp_dir, state) = setup().await;

    let name = "foo";
    let handle0 = state
        .session_create_repository(PathBuf::from(name), None, None, None, false, false, false)
        .await
        .unwrap();

    // Open by full path
    let handle1 = state
        .session_open_repository(
            state
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
    let handle2 = state
        .session_open_repository(PathBuf::from(name), None)
        .await
        .unwrap();
    assert_eq!(handle2, handle0);
}

// TODO: test reopen with higher access mode

#[tokio::test]
async fn expire_empty_repository() {
    test_utils::init_log();

    let (_temp_dir, state) = setup().await;

    let secrets = WriteSecrets::random();

    let name = "foo";
    let handle = state
        .session_create_repository(
            PathBuf::from(name),
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
        .repository_set_block_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();

    state
        .repository_set_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();

    time::sleep(Duration::from_secs(1)).await;

    state.delete_expired_repositories().await;

    assert_matches!(
        state.session_find_repository(name.to_owned()),
        Err(Error::NotFound)
    );
    assert_eq!(
        read_dir(state.store_dir().unwrap(), "").await,
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

    let local_state = State::init(temp_dir.path().join("local/config"))
        .instrument(tracing::info_span!("local"))
        .await
        .unwrap();

    local_state
        .session_set_store_dir(temp_dir.path().join("local/store"))
        .await
        .unwrap();
    local_state
        .session_bind_network(vec![PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
        .await;
    local_state.network.add_user_provided_peer(&remote_addr);

    let name = "foo";
    let handle = local_state
        .session_create_repository(
            PathBuf::from(name),
            None,
            None,
            Some(ShareToken::from(AccessSecrets::Blind { id: secrets.id })),
            true,
            false,
            false,
        )
        .await
        .unwrap();
    let local_repo = local_state.repos.get_repository(handle).unwrap();

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
        .repository_set_block_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();
    local_state
        .repository_set_expiration(handle, Some(Duration::from_millis(100)))
        .await
        .unwrap();

    time::sleep(Duration::from_secs(1)).await;

    local_state.delete_expired_repositories().await;

    assert_matches!(
        local_state.session_find_repository(name.to_owned()),
        Err(Error::NotFound)
    );
    assert_eq!(
        read_dir(local_state.store_dir().unwrap(), "").await,
        Vec::<PathBuf>::new(),
    );
}

#[tokio::test]
async fn move_repository() {
    test_utils::init_log();

    let (_temp_dir, state) = setup().await;
    let src = Path::new("foo");
    let dst = Path::new("bar");

    let repo = state
        .session_create_repository(src.into(), None, None, None, false, false, false)
        .await
        .unwrap();
    let src_full = state.repository_get_path(repo).unwrap();

    let file = state
        .repository_create_file(repo, "test.txt".into())
        .await
        .unwrap();
    state.file_write(file, 0, b"hello".to_vec()).await.unwrap();
    state.file_close(file).await.unwrap();

    state.repository_move(repo, dst.into()).await.unwrap();
    let dst_full = state.repository_get_path(repo).unwrap();

    let file = state
        .repository_open_file(repo, "test.txt".to_owned())
        .await
        .unwrap();
    let len = state.file_get_length(file).unwrap();
    let content = state.file_read(file, 0, len).await.unwrap();
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

#[tokio::test]
async fn delete_repository_with_simple_name() {
    let (_temp_dir, state) = setup().await;

    let name0 = "foo";
    let repo0 = state
        .session_create_repository(PathBuf::from(name0), None, None, None, false, false, false)
        .await
        .unwrap();
    let path0 = state.repository_get_path(repo0).unwrap();

    let name1 = "bar";
    let repo1 = state
        .session_create_repository(PathBuf::from(name1), None, None, None, false, false, false)
        .await
        .unwrap();
    let path1 = state.repository_get_path(repo1).unwrap();

    state.repository_delete(repo0).await.unwrap();

    // Check none of the repo0's files (including the aux files) exist anymore
    assert_eq!(
        read_dir(state.store_dir().unwrap(), path0.to_str().unwrap()).await,
        Vec::<PathBuf>::new()
    );

    // Check the other repo still exists
    assert!(fs::try_exists(path1).await.unwrap());
}

#[tokio::test]
async fn delete_repository_in_subdir_of_store_dir() {
    let (_temp_dir, state) = setup().await;

    let name = "foo/bar/baz";
    let repo = state
        .session_create_repository(PathBuf::from(name), None, None, None, false, false, false)
        .await
        .unwrap();
    let path = state.repository_get_path(repo).unwrap();

    state.repository_delete(repo).await.unwrap();

    assert!(!fs::try_exists(&path).await.unwrap());

    for path in path.ancestors() {
        if path == state.store_dir().unwrap() {
            break;
        }

        assert!(
            !fs::try_exists(path).await.unwrap(),
            "{:?} should not exist",
            path
        );
    }

    assert!(fs::try_exists(state.store_dir().unwrap()).await.unwrap());
}

#[tokio::test]
async fn delete_repository_outside_of_store_dir() {
    let (temp_dir, state) = setup().await;
    let parent_dir = temp_dir.path().join("external");

    let name = "foo";
    let repo = state
        .session_create_repository(parent_dir.join(name), None, None, None, false, false, false)
        .await
        .unwrap();
    let path = state.repository_get_path(repo).unwrap();

    state.repository_delete(repo).await.unwrap();

    assert!(!fs::try_exists(&path).await.unwrap());
    assert!(fs::try_exists(parent_dir).await.unwrap());
}

async fn setup() -> (TempDir, State) {
    let temp_dir = TempDir::new().unwrap();
    let state = State::init(temp_dir.path().join("config")).await.unwrap();
    state
        .session_set_store_dir(temp_dir.path().join("store"))
        .await
        .unwrap();

    (temp_dir, state)
}

/// Collect all files in the directory at `path` whose paths start with `prefix` (using `str`
/// match, not `Path` match!).
async fn read_dir(path: impl AsRef<Path>, prefix: &str) -> Vec<PathBuf> {
    ReadDirStream::new(fs::read_dir(path).await.unwrap())
        .try_filter(|entry| future::ready(entry.path().to_str().unwrap().starts_with(prefix)))
        .map_ok(|entry| entry.path())
        .try_collect::<Vec<_>>()
        .await
        .unwrap()
}

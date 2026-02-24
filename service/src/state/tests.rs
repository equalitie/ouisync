use std::{future, net::Ipv4Addr};

use crate::test_utils;

use super::*;
use assert_matches::assert_matches;
use futures_util::TryStreamExt;
use ouisync::{Access, AccessSecrets, File, Repository, RepositoryParams, WriteSecrets};
use tempfile::TempDir;
use tokio::{sync::broadcast::error::RecvError, time};
use tokio_stream::wrappers::ReadDirStream;
use tracing::Instrument;

#[tokio::test]
async fn normalize_repository_path() {
    let store_dir = PathBuf::from("/home/alice/ouisync");
    let store = Store::new(vec![store_dir.clone()]);

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
                .store_dirs()
                .first()
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
        read_dir(state.store_dirs().first().unwrap(), "").await,
        Vec::<PathBuf>::new()
    );
}

#[tokio::test]
async fn expire_synced_repository() {
    test_utils::init_log();

    let temp_dir = TempDir::new().unwrap();

    let (remote_network, remote_repo, _remote_reg) =
        create_remote_repository(&temp_dir.path().join("remote"))
            .instrument(tracing::info_span!("remote"))
            .await;

    create_file(&remote_repo, "test.txt", "hello world").await;

    let remote_addr = remote_network
        .listener_local_addrs()
        .into_iter()
        .next()
        .unwrap();
    let secrets = remote_repo.secrets();

    let local_state = State::init(ConfigStore::new(temp_dir.path().join("local/config")))
        .instrument(tracing::info_span!("local"))
        .await
        .unwrap();

    local_state
        .session_insert_store_dirs(vec![temp_dir.path().join("local/store")])
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
            Some(ShareToken::from(AccessSecrets::Blind { id: *secrets.id() })),
            true,
            false,
            false,
        )
        .await
        .unwrap();
    let local_repo = local_state.repos.get_repository(handle).unwrap();

    // Wait until synced
    wait_until(&local_repo, async || {
        let progress = local_repo.sync_progress().await.unwrap();
        progress.total > 0 && progress.value == progress.total
    })
    .await;

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
        read_dir(local_state.store_dirs().first().unwrap(), "").await,
        Vec::<PathBuf>::new(),
    );
}

#[cfg(feature = "vfs")]
#[tokio::test]
async fn move_repository() {
    test_utils::init_log();

    // Using a separate TempDir here instead of subdir of `_temp_dir`, otherwise mounting fails with
    // `InvalidMountPoint` on windows for some reason. Also, `mount_dir` must not be dropped before
    // `state` otherwise this test hangs (creating it before `state` ensures this).
    let mount_dir = TempDir::new().unwrap();

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

    // Mount

    state
        .session_set_mount_root(Some(mount_dir.path().into()))
        .await
        .unwrap();
    state.repository_mount(repo).await.unwrap();

    state.repository_move(repo, dst.into()).await.unwrap();
    let dst_full = state.repository_get_path(repo).unwrap();

    // Check the repo now exists at the new path
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

    // Check the repo can be accessed via the API
    let file = state
        .repository_open_file(repo, "test.txt".to_owned())
        .await
        .unwrap();
    let len = state.file_get_length(file).unwrap();
    let content = state.file_read(file, 0, len).await.unwrap();
    assert_eq!(content, b"hello");

    // Check the repo can be accessed via the mountpoint
    let mount_point = state.repository_get_mount_point(repo).unwrap().unwrap();
    let content = fs::read(mount_point.join("test.txt")).await.unwrap();
    assert_eq!(content, b"hello");
}

#[cfg(feature = "vfs")]
#[tokio::test]
async fn attempt_to_move_repository_over_existing_file() {
    test_utils::init_log();

    // Using a separate TempDir here instead of subdir of `_temp_dir`, otherwise mounting fails with
    // `InvalidMountPoint` on windows for some reason. Also, `mount_dir` must not be dropped before
    // `state` otherwise this test hangs (creating it before `state` ensures this).
    let mount_dir = TempDir::new().unwrap();

    let (_temp_dir, state) = setup().await;

    let src = Path::new("foo");
    let dst = Path::new("dst");

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

    // Mount
    state
        .session_set_mount_root(Some(mount_dir.path().into()))
        .await
        .unwrap();
    state.repository_mount(repo).await.unwrap();

    // Create a dummy file at the destination. This prevents the repo from being moved there as it
    // would overwrite the file.
    fs::File::create(
        state
            .session_get_store_dirs()
            .first()
            .unwrap()
            .join(dst)
            .with_extension(REPOSITORY_FILE_EXTENSION),
    )
    .await
    .unwrap();

    assert_matches!(
        state.repository_move(repo, dst.into()).await,
        Err(Error::Io(error)) => {
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists)
        }
    );

    // Check the repo still exists at the original path
    assert_eq!(state.session_list_repositories(), [(src_full, repo)].into());

    // Check the repo can still be accessed via the API
    let file = state
        .repository_open_file(repo, "test.txt".to_owned())
        .await
        .unwrap();
    let len = state.file_get_length(file).unwrap();
    let content = state.file_read(file, 0, len).await.unwrap();
    assert_eq!(content, b"hello");

    // Check the repo can still be accessed via the mountpoint
    let mount_point = state.repository_get_mount_point(repo).unwrap().unwrap();
    let content = fs::read(mount_point.join("test.txt")).await.unwrap();
    assert_eq!(content, b"hello");
}

#[tokio::test]
async fn move_repository_during_sync() {
    test_utils::init_log();

    let temp_dir = TempDir::new().unwrap();

    let secrets = WriteSecrets::random();

    let (remote_network, remote_repo, _remote_reg) = async {
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
    let secrets = remote_repo.secrets();

    let local_state = State::init(ConfigStore::new(temp_dir.path().join("local/config")))
        .instrument(tracing::info_span!("local"))
        .await
        .unwrap();

    local_state
        .session_insert_store_dirs(vec![temp_dir.path().join("local/store")])
        .await
        .unwrap();
    local_state
        .session_bind_network(vec![PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
        .await;
    local_state.network.add_user_provided_peer(&remote_addr);

    let old_name = "foo";
    let new_name = "bar";
    let local_repo_handle = local_state
        .session_create_repository(
            PathBuf::from(old_name),
            None,
            None,
            Some(ShareToken::from(secrets)),
            true,
            false,
            false,
        )
        .await
        .unwrap();

    {
        let local_repo = local_state.repos.get_repository(local_repo_handle).unwrap();
        let file_a = "a.txt";
        let content_a = "aaa";
        create_file(&remote_repo, file_a, content_a).await;
        wait_until(&local_repo, async || {
            check_file_content(&local_repo, file_a, content_a).await
        })
        .await;
    }

    local_state
        .repository_move(local_repo_handle, new_name.into())
        .await
        .unwrap();

    {
        let local_repo = local_state.repos.get_repository(local_repo_handle).unwrap();
        let file_b = "b.txt";
        let content_b = "bbb";
        create_file(&remote_repo, file_b, content_b).await;
        wait_until(&local_repo, async || {
            check_file_content(&local_repo, file_b, content_b).await
        })
        .await;
    }
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
    for store_dir in state.store_dirs() {
        assert_eq!(
            read_dir(store_dir, path0.to_str().unwrap()).await,
            Vec::<PathBuf>::new()
        );
    }

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
        if path == state.store_dirs().first().unwrap() {
            break;
        }

        assert!(
            !fs::try_exists(path).await.unwrap(),
            "{path:?} should not exist"
        );
    }

    for store_dir in state.store_dirs() {
        assert!(fs::try_exists(store_dir).await.unwrap());
    }
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

#[tokio::test]
async fn metrics() {
    let (temp_dir, state) = setup().await;
    let config_dir = temp_dir.path().join("config");

    // Install TLS certificate
    let certs = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();

    fs::write(config_dir.join("cert.pem"), &certs.cert.pem())
        .await
        .unwrap();
    fs::write(
        config_dir.join("key.pem"),
        certs.signing_key.serialize_pem(),
    )
    .await
    .unwrap();

    assert_eq!(state.session_get_metrics_listener_addr(), None);
    state
        .session_bind_metrics(Some((Ipv4Addr::LOCALHOST, 0).into()))
        .await
        .unwrap();
    let addr = state.session_get_metrics_listener_addr().unwrap();
    let url = format!("https://localhost:{}", addr.port());

    let http_client = reqwest::Client::builder()
        .add_root_certificate(reqwest::tls::Certificate::from_der(certs.cert.der()).unwrap())
        .build()
        .unwrap();

    let response = http_client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.content_length().unwrap() > 0);
}

async fn setup() -> (TempDir, State) {
    let temp_dir = TempDir::new().unwrap();
    let state = State::init(ConfigStore::new(temp_dir.path().join("config")))
        .await
        .unwrap();
    state
        .session_insert_store_dirs(vec![temp_dir.path().join("store")])
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

async fn create_remote_repository(root_dir: &Path) -> (Network, Repository, Registration) {
    let secrets = WriteSecrets::random();
    let monitor = StateMonitor::make_root();

    let repo = Repository::create(
        &RepositoryParams::new(root_dir.join("repo.ouisyncdb"))
            .with_monitor(monitor.make_child("repo")),
        Access::WriteUnlocked { secrets },
    )
    .await
    .unwrap();

    let network = Network::new(monitor, None, None);
    network
        .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
        .await;

    let registration = network.register(repo.handle());

    (network, repo, registration)
}

async fn create_file(repo: &Repository, path: &str, content: &str) -> File {
    let mut file = repo.create_file(path).await.unwrap();
    file.write_all(content.as_bytes()).await.unwrap();
    file.flush().await.unwrap();
    file
}

async fn check_file_content(repo: &Repository, path: &str, expected_content: &str) -> bool {
    use ouisync::{Error, StoreError};

    let mut file = match repo.open_file(path).await {
        Ok(file) => file,
        Err(
            Error::EntryNotFound
            | Error::Store(StoreError::BlockNotFound)
            | Error::Store(StoreError::LocatorNotFound),
        ) => return false,
        Err(error) => panic!("unexpected error when opening file: {error:?}"),
    };

    let content = match file.read_to_end().await {
        Ok(content) => content,
        Err(
            Error::Store(StoreError::BlockNotFound) | Error::Store(StoreError::LocatorNotFound),
        ) => return false,
        Err(error) => panic!("unexpected error when reading file: {error:?}"),
    };

    content == expected_content.as_bytes()
}

async fn wait_until<F>(repo: &Repository, mut f: F)
where
    F: AsyncFnMut() -> bool,
{
    let mut rx = repo.subscribe();

    time::timeout(Duration::from_secs(30), async {
        loop {
            if f().await {
                break;
            }

            match rx.recv().await {
                Ok(_) | Err(RecvError::Lagged(_)) => (),
                Err(RecvError::Closed) => panic!("notification channel unexpectedly closed"),
            }
        }
    })
    .await
    .expect("timeout waiting for notification");
}

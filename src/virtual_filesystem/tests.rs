use super::*;
use ouisync::{db, Cryptor, Locator};
use rand::{self, distributions::Standard, rngs::StdRng, Rng, SeedableRng};
use std::{collections::HashMap, ffi::OsString, fs::Metadata, io::ErrorKind};
use tempfile::{tempdir, TempDir};
use tokio::{
    fs::{self, File, OpenOptions},
    io::AsyncWriteExt,
};

#[tokio::test(flavor = "multi_thread")]
async fn empty_repository() {
    let repo = setup().await;
    let (_guard, mount_dir) = mount(repo);

    assert!(read_dir(mount_dir.path()).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn read_directory() {
    let repo = setup().await;
    let mut root_dir = repo.open_directory(Locator::Root).await.unwrap();

    let name_small = OsStr::new("small.txt");
    let len_small = 10;

    let name_large = OsStr::new("large.txt");
    let len_large = 100_000;

    let name_dir = OsStr::new("dir");

    // Small file
    let content: Vec<u8> = rand::thread_rng()
        .sample_iter(Standard)
        .take(len_small)
        .collect();
    let mut file = root_dir.create_file(name_small.into()).unwrap();
    file.write(&content).await.unwrap();
    file.flush().await.unwrap();

    // Large file
    let content: Vec<u8> = rand::thread_rng()
        .sample_iter(Standard)
        .take(len_large)
        .collect();
    let mut file = root_dir.create_file(name_large.into()).unwrap();
    file.write(&content).await.unwrap();
    file.flush().await.unwrap();

    // Subdirectory
    root_dir
        .create_subdirectory(name_dir.into())
        .unwrap()
        .flush()
        .await
        .unwrap();

    root_dir.flush().await.unwrap();

    let (_guard, mount_dir) = mount(repo);

    let entries = read_dir(mount_dir.path()).await;

    assert_eq!(entries.len(), 3);

    assert!(entries[name_small].is_file());
    assert_eq!(entries[name_small].len(), len_small as u64);

    assert!(entries[name_large].is_file());
    assert_eq!(entries[name_large].len(), len_large as u64);

    assert!(entries[name_dir].is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_read_non_existing_directory() {
    let repo = setup().await;
    let (_guard, mount_dir) = mount(repo);

    match fs::read_dir(mount_dir.path().join("missing")).await {
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => panic!("unexpected error {}", error),
        Ok(_) => panic!("unexpected success"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_and_remove_directory() {
    let repo = setup().await;
    let (_guard, mount_dir) = mount(repo);

    fs::create_dir(mount_dir.path().join("dir")).await.unwrap();

    let entries = read_dir(mount_dir.path()).await;
    assert_eq!(entries.len(), 1);
    assert!(entries.contains_key(OsStr::new("dir")));

    fs::remove_dir(mount_dir.path().join("dir")).await.unwrap();
    assert!(read_dir(mount_dir.path()).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_remove_non_empty_directory() {
    let repo = setup().await;
    let (_guard, mount_dir) = mount(repo);

    fs::create_dir(mount_dir.path().join("dir")).await.unwrap();
    fs::write(mount_dir.path().join("dir").join("file.txt"), &[])
        .await
        .unwrap();

    assert!(fs::remove_dir(mount_dir.path().join("dir")).await.is_err());
    assert!(read_dir(mount_dir.path())
        .await
        .contains_key(OsStr::new("dir")));
}

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_empty_file() {
    write_and_read_file_case(0, 0).await
}

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_small_file() {
    write_and_read_file_case(1, 0).await
}

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_large_file() {
    write_and_read_file_case(1024 * 1024, 0).await
}

async fn write_and_read_file_case(len: usize, rng_seed: u64) {
    let repo = setup().await;
    let (_guard, mount_dir) = mount(repo);

    let rng = StdRng::seed_from_u64(rng_seed);
    let orig_data: Vec<u8> = rng.sample_iter(Standard).take(len).collect();

    let path = mount_dir.path().join("file.txt");

    let mut file = File::create(&path).await.unwrap();
    file.write_all(&orig_data).await.unwrap();
    file.sync_all().await.unwrap();

    let read_data = fs::read(path).await.unwrap();

    // Not using `assert_eq!(read_data, orig_data)` to avoid huge output on failure
    assert_eq!(read_data.len(), orig_data.len());
    assert!(read_data == orig_data);
}

#[tokio::test(flavor = "multi_thread")]
async fn append_to_file() {
    let repo = setup().await;
    let (_guard, mount_dir) = mount(repo);

    let path = mount_dir.path().join("file.txt");

    fs::write(&path, b"foo").await.unwrap();

    let mut file = OpenOptions::new().append(true).open(&path).await.unwrap();
    file.write_all(b"bar").await.unwrap();
    file.sync_all().await.unwrap();

    let content = fs::read(path).await.unwrap();
    assert_eq!(content, b"foobar");
}

async fn setup() -> Repository {
    // use std::sync::Once;

    // static LOG_INIT: Once = Once::new();
    // LOG_INIT.call_once(env_logger::init);

    let pool = db::Pool::connect(":memory:").await.unwrap();
    db::create_schema(&pool).await.unwrap();

    Repository::new(pool, Cryptor::Null)
}

fn mount(repository: Repository) -> (MountGuard, TempDir) {
    let mount_dir = tempdir().unwrap();
    let guard = super::mount(
        tokio::runtime::Handle::current(),
        repository,
        mount_dir.path(),
    )
    .unwrap();

    (guard, mount_dir)
}

async fn read_dir(path: impl AsRef<Path>) -> HashMap<OsString, Metadata> {
    let mut entries = HashMap::new();
    let mut stream = fs::read_dir(path).await.unwrap();

    while let Some(entry) = stream.next_entry().await.unwrap() {
        entries.insert(entry.file_name(), entry.metadata().await.unwrap());
    }

    entries
}

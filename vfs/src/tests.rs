use super::*;
use ouisync_lib::{Access, Repository, StateMonitor, WriteSecrets};
use proptest::prelude::*;
use rand::{self, distributions::Standard, rngs::StdRng, Rng, SeedableRng};
use std::{collections::HashMap, ffi::OsString, fs::Metadata, future::Future, io::ErrorKind};
use tempfile::TempDir;
use test_strategy::proptest;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

#[tokio::test(flavor = "multi_thread")]
async fn empty_repository() {
    let (base_dir, _guard) = setup().await;
    assert!(read_dir(base_dir.path().join("mnt")).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn read_directory() {
    let (base_dir, _guard) = setup().await;

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
    fs::write(base_dir.path().join("mnt").join(name_small), &content)
        .await
        .unwrap();

    // Large file
    let content: Vec<u8> = rand::thread_rng()
        .sample_iter(Standard)
        .take(len_large)
        .collect();
    fs::write(base_dir.path().join("mnt").join(name_large), &content)
        .await
        .unwrap();

    // Subdirectory
    fs::create_dir(base_dir.path().join("mnt").join(name_dir))
        .await
        .unwrap();

    let entries = read_dir(base_dir.path().join("mnt")).await;

    assert_eq!(entries.len(), 3);

    assert!(entries[name_small].is_file());
    assert_eq!(entries[name_small].len(), len_small as u64);

    assert!(entries[name_large].is_file());
    assert_eq!(entries[name_large].len(), len_large as u64);

    assert!(entries[name_dir].is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_read_non_existing_directory() {
    let (base_dir, _guard) = setup().await;

    match fs::read_dir(base_dir.path().join("mnt").join("missing")).await {
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => panic!("unexpected error {error}"),
        Ok(_) => panic!("unexpected success"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_and_remove_directory() {
    let (base_dir, _guard) = setup().await;

    fs::create_dir(base_dir.path().join("mnt").join("dir"))
        .await
        .unwrap();

    let entries = read_dir(base_dir.path().join("mnt")).await;
    assert_eq!(entries.len(), 1);
    assert!(entries.contains_key(OsStr::new("dir")));

    fs::remove_dir(base_dir.path().join("mnt").join("dir"))
        .await
        .unwrap();
    assert!(read_dir(base_dir.path().join("mnt")).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_remove_non_empty_directory() {
    let (base_dir, _guard) = setup().await;

    fs::create_dir(base_dir.path().join("mnt").join("dir"))
        .await
        .unwrap();
    fs::write(
        base_dir.path().join("mnt").join("dir").join("file.txt"),
        &[],
    )
    .await
    .unwrap();

    assert!(fs::remove_dir(base_dir.path().join("mnt").join("dir"))
        .await
        .is_err());
    assert!(read_dir(base_dir.path().join("mnt"))
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
    let (base_dir, _guard) = setup().await;

    let rng = StdRng::seed_from_u64(rng_seed);
    let orig_data: Vec<u8> = rng.sample_iter(Standard).take(len).collect();

    let path = base_dir.path().join("mnt").join("file.txt");

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
    let (base_dir, _guard) = setup().await;

    let path = base_dir.path().join("mnt").join("file.txt");

    fs::write(&path, b"foo").await.unwrap();

    let mut file = OpenOptions::new().append(true).open(&path).await.unwrap();
    file.write_all(b"bar").await.unwrap();
    file.sync_all().await.unwrap();

    let content = fs::read(path).await.unwrap();
    assert_eq!(content, b"foobar");
}

#[proptest]
fn seek_and_read(
    #[strategy(0usize..64 * 1024)] len: usize,
    #[strategy(0usize..=#len)] offset: usize,
    #[strategy(any::<u64>().no_shrink())] rng_seed: u64,
) {
    run(seek_and_read_case(len, offset, rng_seed))
}

async fn seek_and_read_case(len: usize, offset: usize, rng_seed: u64) {
    let (base_dir, _guard) = setup().await;

    let path = base_dir.path().join("mnt").join("file.txt");

    let rng = StdRng::seed_from_u64(rng_seed);
    let content: Vec<_> = rng.sample_iter(Standard).take(len).collect();

    fs::write(&path, &content).await.unwrap();

    let mut file = File::open(path).await.unwrap();

    let mut buffer = Vec::new();
    file.seek(SeekFrom::Start(offset as u64)).await.unwrap();
    file.read_to_end(&mut buffer).await.unwrap();

    assert!(buffer == content[offset..]);
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file() {
    let (base_dir, _guard) = setup().await;

    let src_name = OsStr::new("src.txt");
    let src_path = base_dir.path().join("mnt").join(src_name);
    let dst_name = OsStr::new("dst.txt");
    let dst_path = base_dir.path().join("mnt").join(dst_name);

    fs::write(&src_path, b"blah").await.unwrap();
    fs::rename(&src_path, &dst_path).await.unwrap();

    let entries = read_dir(base_dir.path().join("mnt")).await;
    assert!(!entries.contains_key(src_name));
    assert!(entries.contains_key(dst_name));
}

// proptest doesn't work with the `#[tokio::test]` macro yet
// (see https://github.com/AltSysrq/proptest/issues/179). As a workaround, create the runtime
// manually.
fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}

async fn setup() -> (TempDir, MountGuard) {
    use std::thread;
    use tracing::Instrument;

    init_log();

    let base_dir = TempDir::new().unwrap();

    let span = tracing::info_span!("test", name = thread::current().name());
    let monitor = StateMonitor::make_root();

    let repo = Repository::create(
        base_dir.path().join("repo.db"),
        rand::random(),
        Access::WriteUnlocked {
            secrets: WriteSecrets::random(),
        },
        &monitor,
    )
    .instrument(span)
    .await
    .unwrap();
    let repo = Arc::new(repo);

    let mount_dir = base_dir.path().join("mnt");
    fs::create_dir(&mount_dir).await.unwrap();

    let guard = super::mount(tokio::runtime::Handle::current(), repo, mount_dir).unwrap();

    (base_dir, guard)
}

async fn read_dir(path: impl AsRef<Path>) -> HashMap<OsString, Metadata> {
    let mut entries = HashMap::new();
    let mut stream = fs::read_dir(path).await.unwrap();

    while let Some(entry) = stream.next_entry().await.unwrap() {
        entries.insert(entry.file_name(), entry.metadata().await.unwrap());
    }

    entries
}

fn init_log() {
    use tracing::metadata::LevelFilter;
    use tracing_subscriber::EnvFilter;

    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        // log output is captured by default and only shown on failure. Run tests with
        // `--nocapture` to override.
        .with_test_writer()
        .try_init()
        .ok();
}

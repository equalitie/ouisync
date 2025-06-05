use super::*;
use ouisync_lib::{Access, Repository, RepositoryParams, WriteSecrets};
use proptest::prelude::*;
use rand::{self, distributions::Standard, rngs::StdRng, Rng, SeedableRng};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::Metadata,
    io::{ErrorKind, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};
use tempfile::TempDir;
use test_strategy::proptest;
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn empty_repository_single() {
    let setup = Setup::new_single("").await;
    assert!(read_dir(setup.mount_dir_path()).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_repository_multi() {
    let setup = Setup::new_multi("").await;
    assert!(read_dir(setup.mount_dir_path()).await.is_empty());
}

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn read_directory_single() {
    let setup = Setup::new_single("").await;
    read_directory(setup).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn read_directory_multi() {
    let setup = Setup::new_multi("").await;
    read_directory(setup).await;
}

// ----------------------------------

async fn read_directory(setup: Setup) {
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
    fs::write(setup.mount_dir_path().join(name_small), &content)
        .await
        .unwrap();

    // Large file
    let content: Vec<u8> = rand::thread_rng()
        .sample_iter(Standard)
        .take(len_large)
        .collect();
    fs::write(setup.mount_dir_path().join(name_large), &content)
        .await
        .unwrap();

    // Subdirectory
    fs::create_dir(setup.mount_dir_path().join(name_dir))
        .await
        .unwrap();

    let entries = read_dir(setup.mount_dir_path()).await;

    assert_eq!(entries.len(), 3);

    assert!(entries[name_small].is_file());
    assert_eq!(entries[name_small].len(), len_small as u64);

    assert!(entries[name_large].is_file());
    assert_eq!(entries[name_large].len(), len_large as u64);

    assert!(entries[name_dir].is_dir());
}

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_read_non_existing_directory_single() {
    let setup = Setup::new_single("").await;
    attempt_to_read_non_existing_directory(setup).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_read_non_existing_directory_multi() {
    let setup = Setup::new_multi("").await;
    attempt_to_read_non_existing_directory(setup).await;
}

// ----------------------------------

async fn attempt_to_read_non_existing_directory(setup: Setup) {
    match fs::read_dir(setup.mount_dir_path().join("missing")).await {
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => panic!("unexpected error {error}"),
        Ok(_) => panic!("unexpected success"),
    }
}

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn create_and_remove_directory_single() {
    let setup = Setup::new_single("").await;
    create_and_remove_directory(setup).await
}

#[tokio::test(flavor = "multi_thread")]
async fn create_and_remove_directory_multi() {
    let setup = Setup::new_multi("").await;
    create_and_remove_directory(setup).await
}

// ----------------------------------

async fn create_and_remove_directory(setup: Setup) {
    fs::create_dir(setup.mount_dir_path().join("dir"))
        .await
        .unwrap();

    let entries = read_dir(setup.mount_dir_path()).await;
    assert_eq!(entries.len(), 1);
    assert!(entries.contains_key(OsStr::new("dir")));

    fs::remove_dir(setup.mount_dir_path().join("dir"))
        .await
        .unwrap();
    assert!(read_dir(setup.mount_dir_path()).await.is_empty());
}

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_remove_non_empty_directory_single() {
    let setup = Setup::new_single("").await;
    attempt_to_remove_non_empty_directory(setup).await
}

#[tokio::test(flavor = "multi_thread")]
async fn attempt_to_remove_non_empty_directory_multi() {
    let setup = Setup::new_multi("").await;
    attempt_to_remove_non_empty_directory(setup).await
}

// ----------------------------------

async fn attempt_to_remove_non_empty_directory(setup: Setup) {
    fs::create_dir(setup.mount_dir_path().join("dir"))
        .await
        .unwrap();
    fs::write(setup.mount_dir_path().join("dir").join("file.txt"), &[])
        .await
        .unwrap();

    assert!(fs::remove_dir(setup.mount_dir_path().join("dir"))
        .await
        .is_err());
    assert!(read_dir(setup.mount_dir_path())
        .await
        .contains_key(OsStr::new("dir")));
}

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_empty_file_single() {
    write_and_read_file_case_single(0, 0).await
}

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_empty_file_multi() {
    write_and_read_file_case_multi(0, 0).await
}

// ----------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_small_file_single() {
    write_and_read_file_case_single(1, 0).await
}

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_small_file_multi() {
    write_and_read_file_case_multi(1, 0).await
}

// ----------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_large_file_single() {
    write_and_read_file_case_single(1024 * 1024, 0).await
}

#[tokio::test(flavor = "multi_thread")]
async fn write_and_read_large_file_multi() {
    write_and_read_file_case_multi(1024 * 1024, 0).await
}

// ----------------------------------

async fn write_and_read_file_case_single(len: usize, rng_seed: u64) {
    let setup = Setup::new_single(&format!("{len},{rng_seed}")).await;
    write_and_read_file_case(len, rng_seed, setup).await;
}

async fn write_and_read_file_case_multi(len: usize, rng_seed: u64) {
    let setup = Setup::new_multi(&format!("{len},{rng_seed}")).await;
    write_and_read_file_case(len, rng_seed, setup).await;
}

// ----------------------------------

async fn write_and_read_file_case(len: usize, rng_seed: u64, setup: Setup) {
    let rng = StdRng::seed_from_u64(rng_seed);
    let orig_data: Vec<u8> = rng.sample_iter(Standard).take(len).collect();

    let path = setup.mount_dir_path().join("file.txt");

    let mut file = File::create(&path).await.unwrap();
    file.write_all(&orig_data).await.unwrap();
    file.sync_all().await.unwrap();

    let read_data = fs::read(path).await.unwrap();

    // Not using `assert_eq!(read_data, orig_data)` to avoid huge output on failure
    assert_eq!(read_data.len(), orig_data.len());
    assert!(read_data == orig_data);
}

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn append_to_file_single() {
    let setup = Setup::new_single("").await;
    append_to_file(setup).await
}

#[tokio::test(flavor = "multi_thread")]
async fn append_to_file_multi() {
    let setup = Setup::new_multi("").await;
    append_to_file(setup).await
}

// ----------------------------------

async fn append_to_file(setup: Setup) {
    let path = setup.mount_dir_path().join("file.txt");

    fs::write(&path, b"foo").await.unwrap();

    let mut file = OpenOptions::new().append(true).open(&path).await.unwrap();
    file.write_all(b"bar").await.unwrap();
    file.sync_all().await.unwrap();

    let content = fs::read(path).await.unwrap();
    assert_eq!(content, b"foobar");
}

// -----------------------------------------------------------------------------

#[proptest]
fn seek_and_read_single(
    #[strategy(0usize..64 * 1024)] len: usize,
    #[strategy(0usize..=#len)] offset: usize,
    #[strategy(any::<u64>().no_shrink())] rng_seed: u64,
) {
    run(seek_and_read_case_single(len, offset, rng_seed))
}

#[proptest]
fn seek_and_read_multi(
    #[strategy(0usize..64 * 1024)] len: usize,
    #[strategy(0usize..=#len)] offset: usize,
    #[strategy(any::<u64>().no_shrink())] rng_seed: u64,
) {
    run(seek_and_read_case_multi(len, offset, rng_seed))
}

// ----------------------------------

async fn seek_and_read_case_single(len: usize, offset: usize, rng_seed: u64) {
    let setup = Setup::new_single(&format!("{len},{offset},{rng_seed}")).await;
    seek_and_read_case(len, offset, rng_seed, setup).await;
}

async fn seek_and_read_case_multi(len: usize, offset: usize, rng_seed: u64) {
    let setup = Setup::new_multi(&format!("{len},{offset},{rng_seed}")).await;
    seek_and_read_case(len, offset, rng_seed, setup).await;
}

// ----------------------------------

async fn seek_and_read_case(len: usize, offset: usize, rng_seed: u64, setup: Setup) {
    let path = setup.mount_dir_path().join("file.txt");

    let rng = StdRng::seed_from_u64(rng_seed);
    let content: Vec<_> = rng.sample_iter(Standard).take(len).collect();

    fs::write(&path, &content).await.unwrap();

    let mut file = File::open(path).await.unwrap();

    let mut buffer = Vec::new();
    file.seek(SeekFrom::Start(offset as u64)).await.unwrap();
    file.read_to_end(&mut buffer).await.unwrap();

    assert!(buffer == content[offset..]);
}

// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn move_file_single() {
    let setup = Setup::new_single("").await;
    move_file(setup).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn move_file_multi() {
    let setup = Setup::new_multi("").await;
    move_file(setup).await;
}

async fn move_file(setup: Setup) {
    let src_name = OsStr::new("src.txt");
    let src_path = setup.mount_dir_path().join(src_name);
    let dst_name = OsStr::new("dst.txt");
    let dst_path = setup.mount_dir_path().join(dst_name);

    fs::write(&src_path, b"blah").await.unwrap();
    fs::rename(&src_path, &dst_path).await.unwrap();

    let entries = read_dir(setup.mount_dir_path()).await;
    assert!(!entries.contains_key(src_name));
    assert!(entries.contains_key(dst_name));
}

// -----------------------------------------------------------------------------

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

enum MountGuardType {
    Single { _guard: MountGuard },
    Multi { _guard: MultiRepoVFS },
}

struct Setup {
    base_dir: TempDir,
    // NOTE: there is an issue on Windows where all output from FileSystemHandler seems
    // to be redirected to stderr, because of that the log lines are not captured by
    // `cargo test` and therefore log lines from all the tests appear interleaved. Until
    // that issue is fixed, I had to add spans to every test to be able to filter per-test
    // output.
    // https://github.com/dokan-dev/dokan-rust/issues/9
    _span_guard: tracing::span::EnteredSpan,
    mount_guard: MountGuardType,
}

impl Setup {
    async fn new_single(span_params: &str) -> Self {
        init_log();

        let span = Self::create_span(span_params).await;
        let base_dir = TempDir::new().unwrap();
        let store_path = base_dir.path().join("repo.db");
        let repo = Self::create_repo(&store_path, span.clone()).await;
        let mount_dir = base_dir.path().join("mnt");

        fs::create_dir(&mount_dir).await.unwrap();

        #[cfg(target_os = "windows")]
        let mount_guard = super::dokan::single_repo_mount::mount_with_span(
            tokio::runtime::Handle::current(),
            repo,
            mount_dir,
            Some(span.clone()),
        )
        .unwrap();

        #[cfg(not(target_os = "windows"))]
        let mount_guard = super::mount(tokio::runtime::Handle::current(), repo, mount_dir).unwrap();

        // TODO: There is likely a bug in Dokan causing the repository not to appear as mounted righ
        // after the `mount` (or `mount_with_span`) finishes, which makes the tests fail.
        #[cfg(target_os = "windows")]
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        Self {
            base_dir,
            _span_guard: span.entered(),
            mount_guard: MountGuardType::Single {
                _guard: mount_guard,
            },
        }
    }

    async fn new_multi(span_params: &str) -> Self {
        init_log();

        let span = Self::create_span(span_params).await;
        let base_dir = TempDir::new().unwrap();
        let store_path = base_dir.path().join("repo.db");
        let repo = Self::create_repo(&store_path, span.clone()).await;
        let mount_dir = base_dir.path().join("mnt");

        fs::create_dir(&mount_dir).await.unwrap();

        let vfs = MultiRepoVFS::create(mount_dir).await.unwrap();

        vfs.insert("repo".to_owned(), repo).unwrap();

        // TODO: There is likely a bug in Dokan causing the repository not to appear as mounted righ
        // after the `mount` (or `mount_with_span`) finishes, which makes the tests fail.
        #[cfg(target_os = "windows")]
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        Self {
            base_dir,
            _span_guard: span.entered(),
            mount_guard: MountGuardType::Multi { _guard: vfs },
        }
    }

    async fn create_span(span_params: &str) -> tracing::Span {
        tracing::trace_span!(
            "test",
            test_name = format!("{}({span_params})", thread::current().name().unwrap())
        )
    }

    async fn create_repo(store_path: &PathBuf, span: tracing::Span) -> Arc<Repository> {
        use tracing::Instrument;

        let params = RepositoryParams::new(store_path);
        let repo = Repository::create(
            &params,
            Access::WriteUnlocked {
                secrets: WriteSecrets::random(),
            },
        )
        .instrument(span.clone())
        .await
        .unwrap();

        Arc::new(repo)
    }

    fn mount_dir_path(&self) -> PathBuf {
        match self.mount_guard {
            MountGuardType::Single { .. } => self.base_dir.path().join("mnt"),
            MountGuardType::Multi { .. } => self.base_dir.path().join("mnt").join("repo"),
        }
    }
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
        .compact()
        // Target just adds "ouisync_vfs::dokan" to each line.
        .with_target(false)
        .without_time()
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

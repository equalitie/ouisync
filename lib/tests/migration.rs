//! Migration tests

#[macro_use]
mod common;

use async_recursion::async_recursion;
use camino::Utf8Path;
use once_cell::sync::Lazy;
use ouisync::{
    network::Network, Access, AccessMode, AccessSecrets, JointDirectory, JointEntryRef, PeerAddr,
    Repository, RepositoryParams, StateMonitor, DATA_VERSION, DIRECTORY_VERSION, SCHEMA_VERSION,
};
use rand::{
    distributions::{Alphanumeric, DistString, Standard},
    prelude::Distribution,
    rngs::StdRng,
    Rng, SeedableRng,
};
use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fmt, io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    str,
};
use tempfile::TempDir;
use tokio::{fs, process::Command};
use tracing::{instrument, Instrument};

const DB_EXTENSION: &str = "db";
const DB_DUMP_EXTENSION: &str = "db.dump";

static DUMP: Lazy<DumpDirectory> = Lazy::new(|| {
    DumpDirectory::new()
        .add("empty.txt", vec![])
        .add("small.txt", b"foo".to_vec())
        .add("large.txt", seeded_random_bytes(0, 128 * 1024))
        .add("dir-a", DumpDirectory::new())
        .add(
            "dir-b",
            DumpDirectory::new()
                .add("file-in-dir-b.txt", b"bar".to_vec())
                .add("subdir", DumpDirectory::new()),
        )
});

/// Runs all tests on all db dumps.
#[tokio::test(flavor = "multi_thread")]
async fn suite() {
    common::init_log();

    let root_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dump_dir = root_dir.join("tests/dumps");
    let work_dir = TempDir::new().unwrap();

    let current_dump_name = format!(
        "{}-{}-{}.{}",
        *SCHEMA_VERSION, DATA_VERSION, DIRECTORY_VERSION, DB_DUMP_EXTENSION
    );
    let current_dump_path = dump_dir.join(&current_dump_name);

    if env::var("TEST_MIGRATION_DUMP").is_ok() {
        create_repo_dump(work_dir.path(), &current_dump_path).await;
    }

    if !fs::try_exists(&current_dump_path).await.unwrap() {
        panic!(
            "Latest version dump '{}' not found. \
             Rerun this test with env variable `TEST_MIGRATION_DUMP=1`, \
             then check the dump into the source control.",
            current_dump_path.display(),
        );
    }

    let mut dumps = fs::read_dir(dump_dir).await.unwrap();

    while let Some(entry) = dumps.next_entry().await.unwrap() {
        let path = entry.path();

        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(&format!(".{DB_DUMP_EXTENSION}")))
            .unwrap_or(false)
        {
            continue;
        }

        group(work_dir.path(), &path).await;
    }
}

/// Runs all tests on the given db dump.
#[instrument(skip(work_dir))]
async fn group(work_dir: &Path, input_dump: &Path) {
    let (schema_version, data_version, directory_version) = parse_versions(input_dump);

    info!(schema_version, data_version, directory_version, "start");

    test_load_writer(work_dir, input_dump).await;
    test_load_reader(work_dir, input_dump).await;
    test_sync(work_dir, input_dump).await;

    info!(schema_version, data_version, directory_version, "done");
}

#[instrument(skip_all)]
async fn test_load_writer(work_dir: &Path, input_dump: &Path) {
    info!("start");

    let repo = load_repo(work_dir, input_dump, AccessMode::Write).await;
    let dump = dump(&repo).await;
    similar_asserts::assert_eq!(dump, *DUMP);

    info!("done");
}

#[instrument(skip_all)]
async fn test_load_reader(work_dir: &Path, input_dump: &Path) {
    info!("start");

    let repo = load_repo(work_dir, input_dump, AccessMode::Read).await;
    let dump = dump(&repo).await;
    similar_asserts::assert_eq!(dump, *DUMP);

    info!("done");
}

#[instrument(skip_all)]
async fn test_sync(work_dir: &Path, input_dump: &Path) {
    info!("start");

    let (repo_a, network_a) = async {
        let repo = load_repo(work_dir, input_dump, AccessMode::Write).await;
        let network = create_network().await;
        (repo, network)
    }
    .instrument(info_span!("a"))
    .await;

    let (repo_b, network_b) = async {
        let repo = Repository::create(
            &RepositoryParams::new(make_store_path(work_dir)).with_device_id(seeded_random(1)),
            Access::new(None, None, repo_a.secrets().clone()),
        )
        .await
        .unwrap();
        let network = create_network().await;
        (repo, network)
    }
    .instrument(info_span!("b"))
    .await;

    let _reg_a = network_a.register(repo_a.handle()).await;
    let _reg_b = network_b.register(repo_b.handle()).await;

    network_b.add_user_provided_peer(&network_a.listener_local_addrs().into_iter().next().unwrap());

    wait_until_sync(&repo_b, &repo_a).await;

    let dump = dump(&repo_b).await;
    similar_asserts::assert_eq!(dump, *DUMP);

    info!("done");
}

async fn create_network() -> Network {
    let network = Network::new(None, StateMonitor::make_root());
    network
        .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
        .await;
    network
}

async fn wait_until_sync(a: &Repository, b: &Repository) {
    common::eventually(a, || async {
        let progress = a.sync_progress().await.unwrap();
        debug!(progress = %progress.percent());

        if progress.total == 0 || progress.value < progress.total {
            return false;
        }

        let vv_a = a.local_branch().unwrap().version_vector().await.unwrap();
        let vv_b = b.local_branch().unwrap().version_vector().await.unwrap();
        debug!(?vv_a, ?vv_b);

        vv_a == vv_b
    })
    .await
}

#[instrument(skip(work_dir))]
async fn create_repo_dump(work_dir: &Path, output_path: &Path) {
    let store_path = work_dir
        .join(output_path.file_stem().unwrap())
        .with_extension(DB_EXTENSION);

    // Create the repo
    let secrets = AccessSecrets::random_write();
    let access = Access::new(None, None, secrets);
    let repo = Repository::create(
        &RepositoryParams::new(&store_path).with_device_id(seeded_random(0)),
        access,
    )
    .await
    .unwrap();

    // Populate it with data
    populate(&repo, &DUMP).await;

    repo.close().await.unwrap();

    // Dump the repo database to a gzipped sql file
    save_db_dump(&store_path, output_path).await;
}

async fn load_repo(work_dir: &Path, input_dump: &Path, access_mode: AccessMode) -> Repository {
    let store_path = make_store_path(work_dir);

    load_db_dump(&store_path, input_dump).await;

    Repository::open(
        &RepositoryParams::new(store_path).with_device_id(seeded_random(0)),
        None,
        access_mode,
    )
    .await
    .unwrap()
}

fn make_store_path(work_dir: &Path) -> PathBuf {
    work_dir
        .join(Alphanumeric.sample_string(&mut rand::thread_rng(), 8))
        .with_extension(DB_EXTENSION)
}

async fn save_db_dump(db_path: &Path, dump_path: &Path) {
    fs::create_dir_all(dump_path.parent().unwrap())
        .await
        .unwrap();

    match fs::remove_file(dump_path).await {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => panic!("{error:?}"),
    }

    let mut command = OsString::new();
    command.push("VACUUM INTO '");
    command.push(dump_path);
    command.push("'");

    let status = Command::new("sqlite3")
        .arg(db_path)
        .arg(command)
        .status()
        .await
        .unwrap();

    assert!(status.success());
}

async fn load_db_dump(db_path: &Path, dump_path: &Path) {
    fs::copy(dump_path, db_path).await.unwrap();
}

fn parse_versions(dump_path: &Path) -> (u32, u32, u32) {
    let mut parts = dump_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_suffix(&format!(".{DB_DUMP_EXTENSION}"))
        .unwrap()
        .split('-')
        .map(|part| part.parse().unwrap_or(0))
        .fuse();

    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[derive(Eq, PartialEq, Debug)]
struct DumpDirectory(BTreeMap<String, DumpEntry>);

impl DumpDirectory {
    const fn new() -> Self {
        Self(BTreeMap::new())
    }

    fn add(mut self, name: impl Into<String>, entry: impl Into<DumpEntry>) -> Self {
        self.0.insert(name.into(), entry.into());
        self
    }
}

#[derive(Eq, PartialEq)]
struct DumpFile(Vec<u8>);

impl fmt::Debug for DumpFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = str::from_utf8(&self.0) {
            write!(f, "{s:?}")
        } else {
            write!(f, "{}", hex::encode(&self.0))
        }
    }
}

#[derive(Eq, PartialEq, Debug)]
enum DumpEntry {
    File(DumpFile),
    Directory(DumpDirectory),
}

impl From<Vec<u8>> for DumpEntry {
    fn from(content: Vec<u8>) -> Self {
        Self::File(DumpFile(content))
    }
}

impl From<DumpDirectory> for DumpEntry {
    fn from(dir: DumpDirectory) -> Self {
        Self::Directory(dir)
    }
}

async fn dump(repo: &Repository) -> DumpDirectory {
    dump_directory(repo.open_directory("/").await.unwrap()).await
}

#[async_recursion]
async fn dump_directory(dir: JointDirectory) -> DumpDirectory {
    let mut entries = BTreeMap::new();

    for entry in dir.entries() {
        let name = entry.name().to_owned();
        let dump = match entry {
            JointEntryRef::File(entry) => {
                let mut file = entry.open().await.unwrap();
                DumpEntry::File(DumpFile(file.read_to_end().await.unwrap()))
            }
            JointEntryRef::Directory(entry) => {
                let dir = entry.open().await.unwrap();
                DumpEntry::Directory(dump_directory(dir).await)
            }
        };

        entries.insert(name, dump);
    }

    DumpDirectory(entries)
}

async fn populate(repo: &Repository, dump: &DumpDirectory) {
    populate_directory(repo, Utf8Path::new("/"), dump).await
}

#[async_recursion]
async fn populate_directory(repo: &Repository, path: &Utf8Path, dump: &DumpDirectory) {
    for (name, dump) in &dump.0 {
        let path = path.join(name);

        match dump {
            DumpEntry::File(dump) => {
                let mut file = repo.create_file(path).await.unwrap();
                file.write_all(&dump.0).await.unwrap();
                file.flush().await.unwrap();
            }
            DumpEntry::Directory(dump) => {
                repo.create_directory(&path).await.unwrap();
                populate_directory(repo, &path, dump).await;
            }
        }
    }
}

fn seeded_random_bytes(seed: u64, size: usize) -> Vec<u8> {
    StdRng::seed_from_u64(seed)
        .sample_iter(Standard)
        .take(size)
        .collect()
}

fn seeded_random<T>(seed: u64) -> T
where
    Standard: Distribution<T>,
{
    StdRng::seed_from_u64(seed).gen()
}

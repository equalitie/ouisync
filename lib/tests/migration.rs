//! Migration tests

#[macro_use]
mod common;

use common::{dump, sync_watch};
use futures_util::future;
use once_cell::sync::Lazy;
use ouisync::{
    Access, AccessMode, AccessSecrets, Network, PeerAddr, Repository, RepositoryParams,
    DATA_VERSION, DIRECTORY_VERSION, SCHEMA_VERSION,
};
use rand::{
    distributions::{Alphanumeric, DistString, Standard},
    prelude::Distribution,
    rngs::StdRng,
    Rng, SeedableRng,
};
use state_monitor::StateMonitor;
use std::{
    env,
    ffi::OsString,
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    str,
};
use tempfile::TempDir;
use tokio::{fs, process::Command};
use tracing::{instrument, Instrument};

const DB_EXTENSION: &str = "db";
const DB_DUMP_EXTENSION: &str = "db.dump";

static DUMP: Lazy<dump::Directory> = Lazy::new(|| {
    dump::Directory::new()
        .add("empty.txt", vec![])
        .add("small.txt", b"foo".to_vec())
        .add("large.txt", seeded_random_bytes(0, 128 * 1024))
        .add("dir-a", dump::Directory::new())
        .add(
            "dir-b",
            dump::Directory::new()
                .add("file-in-dir-b.txt", b"bar".to_vec())
                .add("subdir", dump::Directory::new()),
        )
});

/// Runs all tests on all db dumps.
#[tokio::test(flavor = "multi_thread")]
async fn suite() {
    common::init_log();

    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
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
#[instrument(target = "ouisync-test", skip(work_dir))]
async fn group(work_dir: &Path, input_dump: &Path) {
    let (schema_version, data_version, directory_version) = parse_versions(input_dump);

    info!(schema_version, data_version, directory_version, "start");

    test_load_writer(work_dir, input_dump).await;
    test_load_reader(work_dir, input_dump).await;
    test_sync(work_dir, input_dump).await;

    info!(schema_version, data_version, directory_version, "done");
}

#[instrument(target = "ouisync-test", skip_all)]
async fn test_load_writer(work_dir: &Path, input_dump: &Path) {
    info!("start");

    let repo = load_repo(work_dir, input_dump, AccessMode::Write).await;
    assert!(repo.check_integrity().await.unwrap());

    let dump = dump::save(&repo).await;
    similar_asserts::assert_eq!(dump, *DUMP);

    info!("done");
}

#[instrument(target = "ouisync-test", skip_all)]
async fn test_load_reader(work_dir: &Path, input_dump: &Path) {
    info!("start");

    let repo = load_repo(work_dir, input_dump, AccessMode::Read).await;
    assert!(repo.check_integrity().await.unwrap());

    let dump = dump::save(&repo).await;
    similar_asserts::assert_eq!(dump, *DUMP);

    info!("done");
}

#[instrument(target = "ouisync-test", skip_all)]
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

    // Wait until B fully syncs with A
    let (tx, rx) = sync_watch::channel();
    future::join(tx.run(&repo_a), rx.run(&repo_b)).await;

    let dump = dump::save(&repo_b).await;
    similar_asserts::assert_eq!(dump, *DUMP);

    info!("done");
}

async fn create_network() -> Network {
    let network = Network::new(StateMonitor::make_root(), None, None);
    network
        .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
        .await;
    network
}

#[instrument(target = "ouisync-test", skip(work_dir))]
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
    dump::load(&repo, &DUMP).await;

    repo.close().await.unwrap();

    // Dump the repo database to a gzipped sql file
    save_db_dump(&store_path, output_path).await;
}

async fn load_repo(work_dir: &Path, input_dump: &Path, access_mode: AccessMode) -> Repository {
    let store_path = make_store_path(work_dir);

    load_db_dump(&store_path, input_dump).await;

    Repository::open(&RepositoryParams::new(store_path), None, access_mode)
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

fn seeded_random_bytes(seed: u64, size: usize) -> Vec<u8> {
    StdRng::seed_from_u64(seed)
        // The `u8` should be inferred but for some reason it doesn't work when compiling on
        // windows...
        .sample_iter::<u8, _>(Standard)
        .take(size)
        .collect()
}

fn seeded_random<T>(seed: u64) -> T
where
    Standard: Distribution<T>,
{
    StdRng::seed_from_u64(seed).gen()
}

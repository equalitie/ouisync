#![allow(unused)] // https://github.com/rust-lang/rust/issues/46379

#[macro_use]
mod macros;

pub(crate) mod dump;
pub(crate) mod progress;
pub(crate) mod sync_watch;
pub(crate) mod traffic_monitor;
mod wait_map;

pub(crate) use self::env::*;

use self::wait_map::WaitMap;
use camino::Utf8Path;
use metrics::{Label, NoopRecorder, Recorder};
use metrics_ext::{WatchRecorder, WatchRecorderSubscriber};
use once_cell::sync::Lazy;
use ouisync::{
    crypto::sign::PublicKey, Access, AccessSecrets, DeviceId, EntryType, Error, Event, File,
    Network, Payload, PeerAddr, Registration, Repository, Result, StoreError,
};
use ouisync_tracing_fmt::Formatter;
use rand::Rng;
use state_monitor::StateMonitor;
use std::{
    fmt,
    future::Future,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    thread,
};
use tokio::{
    runtime::Handle,
    sync::broadcast::{self, error::RecvError},
    task_local,
    time::{self, Duration},
};
use tracing::metadata::LevelFilter;
use tracing::{instrument, Instrument, Span};
use tracing_subscriber::{
    fmt::{time::SystemTime, MakeWriter, TestWriter},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Layer,
};

pub(crate) const DEFAULT_REPO: &str = "default";

// Timeout for waiting for an event. Can be overwritten using "TEST_EVENT_TIMEOUT" env variable
// (in seconds).
pub(crate) static EVENT_TIMEOUT: Lazy<Duration> = Lazy::new(|| {
    Duration::from_secs(
        std::env::var("TEST_EVENT_TIMEOUT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(60),
    )
});

pub(crate) static TEST_TIMEOUT: Lazy<Duration> = Lazy::new(|| 4 * *EVENT_TIMEOUT);

#[cfg(not(feature = "simulation"))]
pub(crate) mod env {
    use super::*;
    use futures_util::future;
    use tokio::{
        runtime::{self, Runtime},
        sync::oneshot,
        task::JoinHandle,
    };

    /// Test environment that uses real network (localhost)
    pub(crate) struct Env {
        context: Arc<Context>,
        runtime: Runtime,
        tasks: Vec<JoinHandle<()>>,
    }

    impl Env {
        pub fn new() -> Self {
            let runtime = runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();

            let context = Context::new(runtime.handle());

            Self {
                context: Arc::new(context),
                runtime,
                tasks: Vec::new(),
            }
        }

        pub fn actor<Fut>(&mut self, name: &str, f: Fut)
        where
            Fut: Future<Output = ()> + Send + 'static,
        {
            let actor = Actor::new(name.to_owned(), self.context.clone());
            let span = info_span!("actor", message = name);

            let f = ACTOR.scope(actor, f);
            let f = f.instrument(span);

            self.tasks.push(self.runtime.spawn(f));
        }

        pub fn runtime(&self) -> Handle {
            self.runtime.handle().clone()
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            self.runtime
                .block_on(future::try_join_all(self.tasks.drain(..)))
                .unwrap();
        }
    }
}

#[cfg(feature = "simulation")]
pub(crate) mod env {
    use super::*;

    /// Test environment that uses simulated network
    pub(crate) struct Env<'a> {
        context: Arc<Context>,
        runner: turmoil::Sim<'a>,
    }

    impl<'a> Env<'a> {
        pub fn new() -> Self {
            let context = Context::new(&Handle::current());
            let runner = turmoil::Builder::new()
                .simulation_duration(Duration::from_secs(90))
                .build_with_rng(Box::new(rand::thread_rng()));

            Self {
                context: Arc::new(context),
                runner,
            }
        }

        pub fn actor<Fut>(&mut self, name: &str, f: Fut)
        where
            Fut: Future<Output = ()> + 'static,
        {
            let actor = Actor::new(name.to_owned(), self.context.clone());
            let span = info_span!("actor", message = name);

            let f = async move {
                f.await;
                Ok(())
            };
            let f = ACTOR.scope(actor, f);
            let f = f.instrument(span);

            self.runner.client(name, f);
        }

        pub fn runtime(&self) -> Handle {
            unimplemented!("not supported in simulation")
        }
    }

    impl Drop for Env<'_> {
        fn drop(&mut self) {
            self.runner.run().unwrap()
        }
    }
}

/// Operations on the current actor. All of these function can only be called in an actor context,
/// that is, from inside the future passed to `Env::actor`.
pub(crate) mod actor {
    use super::*;
    use metrics::Key;
    use metrics_ext::Pair;
    use ouisync::{
        crypto::{sign::Keypair, Hash, Hashable},
        AccessMode, RepositoryParams,
    };
    use state_monitor::StateMonitor;
    use tokio::sync::watch;

    pub(crate) fn name() -> String {
        ACTOR.with(|actor| actor.name.clone())
    }

    pub(crate) fn create_unbound_network() -> Network {
        // Derive runtime id from the actor name so that the runtime ids don't change across
        // multiple test invocation. This simplifies debugging.
        let runtime_id = Keypair::try_from(name().as_bytes().hash().as_ref())
            .unwrap()
            .into();

        Network::new(StateMonitor::make_root(), None, Some(runtime_id))
    }

    pub(crate) async fn create_network(proto: Proto) -> Network {
        let network = create_unbound_network();
        bind(&network, proto).await;
        network
    }

    pub(crate) async fn bind(network: &Network, proto: Proto) {
        let bind_addr = proto.wrap((Ipv4Addr::UNSPECIFIED, 0));
        network.bind(&[bind_addr]).await;

        let bind_addr = network
            .listener_local_addrs()
            .into_iter()
            .find(|addr| Proto::of(addr) == proto)
            .unwrap();
        register_addr(bind_addr);
    }

    pub(crate) fn register_addr(addr: PeerAddr) {
        ACTOR.with(|actor| {
            actor.context.addr_map.insert(actor.name.clone(), addr);
        })
    }

    pub(crate) async fn lookup_addr(name: &str) -> PeerAddr {
        let context = ACTOR.with(|actor| actor.context.clone());
        let addr = context.addr_map.get(name).await;

        fn unspecified_to_localhost(addr: SocketAddr) -> SocketAddr {
            let ip = addr.ip();
            let ip = match ip {
                IpAddr::V4(addr) if addr.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
                IpAddr::V6(addr) if addr.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
                IpAddr::V4(_) | IpAddr::V6(_) => ip,
            };

            SocketAddr::new(ip, addr.port())
        }

        match addr {
            PeerAddr::Quic(addr) => PeerAddr::Quic(unspecified_to_localhost(addr)),
            PeerAddr::Tcp(addr) => PeerAddr::Tcp(unspecified_to_localhost(addr)),
        }
    }

    pub(crate) fn get_repo_params(name: &str) -> RepositoryParams<NoopRecorder> {
        ACTOR.with(|actor| {
            RepositoryParams::new(actor.repo_path(name))
                .with_device_id(actor.device_id)
                .with_parent_monitor(actor.monitor.clone())
                .with_recorder(NoopRecorder)
        })
    }

    pub(crate) fn get_default_recorder() -> DefaultRecorder {
        ACTOR.with(|actor| {
            metrics_ext::AddLabels::new(
                vec![Label::new("actor", actor.name.clone())],
                actor.context.recorder.clone(),
            )
        })
    }

    pub(crate) type DefaultRecorder = metrics_ext::AddLabels<metrics_ext::Shared>;

    pub(crate) fn get_repo_secrets(name: &str) -> AccessSecrets {
        ACTOR.with(|actor| {
            actor
                .context
                .repo_map
                .get_or_insert_with(name.to_owned(), AccessSecrets::random_write)
        })
    }

    pub(crate) fn get_repo_path(name: &str) -> PathBuf {
        ACTOR.with(|actor| actor.repo_path(name))
    }

    pub(crate) async fn create_repo_with_mode(name: &str, mode: AccessMode) -> Repository {
        let params = get_repo_params(name);
        let secrets = get_repo_secrets(name);

        Repository::create(&params, Access::new(None, None, secrets.with_mode(mode)))
            .await
            .unwrap()
    }

    pub(crate) async fn create_repo(name: &str) -> Repository {
        create_repo_with_mode(name, AccessMode::Write).await
    }

    pub(crate) async fn create_linked_repo(
        name: &str,
        network: &Network,
    ) -> (Repository, Registration) {
        let repo = create_repo(name).await;
        let reg = network.register(repo.handle()).await;

        (repo, reg)
    }

    /// Convenience function for the common case where the actor has one linked repository.
    pub(crate) async fn setup() -> (Network, Repository, Registration) {
        let network = create_network(Proto::Tcp).await;
        let (repo, reg) = create_linked_repo(DEFAULT_REPO, &network).await;
        (network, repo, reg)
    }
}

task_local! {
    static ACTOR: Actor;
}

struct Context {
    base_dir: TempDir,
    addr_map: WaitMap<String, PeerAddr>,
    repo_map: WaitMap<String, AccessSecrets>,
    recorder: metrics_ext::Shared,
    monitor: StateMonitor,
}

impl Context {
    fn new(runtime: &Handle) -> Self {
        init_log();

        let recorder = init_recorder(runtime);

        Self {
            base_dir: TempDir::new(),
            addr_map: WaitMap::new(),
            repo_map: WaitMap::new(),
            recorder,
            monitor: StateMonitor::make_root(),
        }
    }
}

struct Actor {
    name: String,
    context: Arc<Context>,
    base_dir: PathBuf,
    device_id: DeviceId,
    monitor: StateMonitor,
    recorder: WatchRecorder,
}

impl Actor {
    fn new(name: String, context: Arc<Context>) -> Self {
        let base_dir = context.base_dir.path().join(&name);
        let monitor = context.monitor.make_child(&name);
        let recorder = WatchRecorder::new();

        Actor {
            name,
            context,
            base_dir,
            device_id: rand::random(),
            monitor,
            recorder,
        }
    }

    fn repo_path(&self, name: &str) -> PathBuf {
        self.base_dir.join(name).with_extension("db")
    }
}

/// Wrapper for `tempfile::TempDir` which preserves the dir in case of panic.
struct TempDir(Option<tempfile::TempDir>);

impl TempDir {
    fn new() -> Self {
        Self(Some(tempfile::TempDir::new().unwrap()))
    }

    fn path(&self) -> &Path {
        self.0.as_ref().unwrap().path()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Preserve the dir in case of panic, so it can be inspected to help debug test
        // failures.
        if thread::panicking() {
            let path = self.0.take().unwrap().into_path();
            warn!("preserving temp dir in '{}'", path.display());
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum Proto {
    Tcp,
    Quic,
}

impl Proto {
    pub fn wrap(&self, addr: impl Into<SocketAddr>) -> PeerAddr {
        match self {
            Self::Tcp => PeerAddr::Tcp(addr.into()),
            Self::Quic => PeerAddr::Quic(addr.into()),
        }
    }

    pub fn of(addr: &PeerAddr) -> Self {
        match addr {
            PeerAddr::Quic(_) => Self::Quic,
            PeerAddr::Tcp(_) => Self::Tcp,
        }
    }
}

impl FromStr for Proto {
    type Err = ProtoParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input.trim().to_lowercase().as_str() {
            "tcp" => Ok(Self::Tcp),
            "quic" => Ok(Self::Quic),
            _ => Err(ProtoParseError),
        }
    }
}

impl fmt::Display for Proto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp => write!(f, "TCP"),
            Self::Quic => write!(f, "QUIC"),
        }
    }
}

#[derive(Debug)]
pub struct ProtoParseError;

impl fmt::Display for ProtoParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to parse protocol")
    }
}

impl std::error::Error for ProtoParseError {}

// Keep calling `f` until it returns `true`. Wait for repo notification between calls.

pub(crate) async fn eventually<F, Fut>(repo: &Repository, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let mut rx = repo.subscribe();

    time::timeout(*TEST_TIMEOUT, async {
        loop {
            if f().await {
                break;
            }

            wait(&mut rx).await;
        }
    })
    .await
    .unwrap()
}

/// Waits for an event to be received and returns it. Returns `None` if the received lagged.
pub(crate) async fn wait(rx: &mut broadcast::Receiver<Event>) -> Option<Payload> {
    match time::timeout(*EVENT_TIMEOUT, rx.recv()).await {
        Ok(event) => {
            debug!(?event);

            match event {
                Ok(event) => Some(event.payload),
                Err(RecvError::Lagged(_)) => None,
                Err(RecvError::Closed) => panic!("notification channel unexpectedly closed"),
            }
        }
        Err(_) => {
            const MESSAGE: &str = "timeout waiting for notification";

            // NOTE: in release mode backtrace is useless so this trace helps us to locate the
            // source of the panic:
            error!("{}", MESSAGE);
            panic!("{}", MESSAGE);
        }
    }
}

/// Wait until the file at `path` has the expected content. Panics if timeout elapses before the
/// file content matches.

pub(crate) async fn expect_file_content(repo: &Repository, path: &str, expected_content: &[u8]) {
    expect_file_version_content(repo, path, None, expected_content).await
}

/// Wait until the file as `path` is in the local branch and has the expected content.
#[allow(unused)]
pub(crate) async fn expect_local_file_content(
    repo: &Repository,
    path: &str,
    expected_content: &[u8],
) {
    let local_branch = repo.local_branch().unwrap();
    expect_file_version_content(repo, path, Some(local_branch.id()), expected_content).await
}

#[instrument(skip(repo, expected_content))]
pub(crate) async fn expect_file_version_content(
    repo: &Repository,
    path: &str,
    branch_id: Option<&PublicKey>,
    expected_content: &[u8],
) {
    eventually(repo, || {
        check_file_version_content(repo, path, branch_id, expected_content)
            .instrument(Span::current())
    })
    .await
}

pub(crate) async fn check_file_version_content(
    repo: &Repository,
    path: &str,
    branch_id: Option<&PublicKey>,
    expected_content: &[u8],
) -> bool {
    let Some(mut file) = open_file_version(repo, path, branch_id).await else {
        return false;
    };

    // For large files this is faster than reading the file content and compare.
    if expected_content.len() as u64 != file.len() {
        return false;
    }

    let actual_content = match read_in_chunks(&mut file, 4096).await {
        Ok(content) => content,
        // `BlockNotFound` means just that some block of the file hasn't been downloaded yet.
        // `LocatorNotFound` likely means we've received a partially merged branch and a block
        // referenced by a blob is not yet referenced by that branch.
        Err(
            error @ (Error::Store(StoreError::BlockNotFound)
            | Error::Store(StoreError::LocatorNotFound)),
        ) => {
            warn!(path, ?error, "read failed");
            return false;
        }
        Err(error) => {
            error!(path, ?error);
            panic!("unexpected error: {error:?}");
        }
    };

    if actual_content == expected_content {
        debug!(path, "content matches");
        true
    } else {
        warn!(path, "content does not match");
        false
    }
}

pub(crate) async fn open_file_version(
    repo: &Repository,
    path: &str,
    branch_id: Option<&PublicKey>,
) -> Option<File> {
    debug!(path, "opening");

    let result = if let Some(branch_id) = branch_id {
        repo.open_file_version(path, branch_id).await
    } else {
        repo.open_file(path).await
    };

    let file = match result {
        Ok(file) => file,
        // - `EntryNotFound` likely means that the parent directory hasn't yet been fully synced
        //    and so the file entry is not in it yet.
        // - `BlockNotFound` means the first block of the file hasn't been downloaded yet.
        // - `LocatorNotFound` TODO: it seems the tests pass when we allow it and so might be ok
        //    but we need to confirm it and understand how it happens.
        Err(
            error @ (Error::EntryNotFound
            | Error::Store(StoreError::BlockNotFound)
            | Error::Store(StoreError::LocatorNotFound)),
        ) => {
            warn!(path, ?branch_id, ?error, "open failed");
            return None;
        }
        Err(error) => {
            error!(path, ?branch_id, ?error);
            panic!("unexpected error: {error:?}");
        }
    };

    debug!(path, branch.id = ?file.branch().id(), "opened");

    Some(file)
}

#[instrument(skip(repo))]
pub(crate) async fn expect_entry_exists(repo: &Repository, path: &str, entry_type: EntryType) {
    eventually(repo, || check_entry_exists(repo, path, entry_type)).await
}

pub(crate) async fn check_entry_exists(
    repo: &Repository,
    path: &str,
    entry_type: EntryType,
) -> bool {
    debug!(path, "opening");

    let result = match entry_type {
        EntryType::File => repo.open_file(path).await.map(|_| ()),
        EntryType::Directory => repo.open_directory(path).await.map(|_| ()),
    };

    match result {
        Ok(()) => {
            debug!(path, "opened");
            true
        }
        Err(
            error @ (Error::EntryNotFound
            | Error::Store(StoreError::BlockNotFound)
            | Error::Store(StoreError::LocatorNotFound)),
        ) => {
            warn!(path, ?error, "open failed");
            false
        }
        Err(error) => {
            error!(path, ?error);
            panic!("unexpected error: {error:?}");
        }
    }
}

#[instrument(skip(repo))]
pub(crate) async fn expect_entry_not_found(repo: &Repository, path: &str) {
    let path = Utf8Path::new(path);
    let name = path.file_name().unwrap();
    let parent = path.parent().unwrap();

    eventually(repo, || async {
        let parent = repo.open_directory(parent).await.unwrap();

        match parent.lookup_unique(name) {
            Ok(_) => {
                debug!(%path, "still exists");
                false
            }
            Err(Error::EntryNotFound) => true,
            Err(error) => {
                error!(%path, ?error);
                panic!("unexpected error: {error:?}");
            }
        }
    })
    .await
}

pub(crate) async fn write_in_chunks(file: &mut File, content: &[u8], chunk_size: usize) {
    for offset in (0..content.len()).step_by(chunk_size) {
        let end = (offset + chunk_size).min(content.len());
        file.write_all(&content[offset..end]).await.unwrap();

        if to_megabytes(end) > to_megabytes(offset) {
            debug!(
                "file write progress: {}/{} MB",
                to_megabytes(end),
                to_megabytes(content.len())
            );
        }
    }
}

pub(crate) async fn read_in_chunks(file: &mut File, chunk_size: usize) -> Result<Vec<u8>, Error> {
    let mut content = vec![0; file.len() as usize];
    let mut offset = 0;

    while offset < content.len() {
        let end = (offset + chunk_size).min(content.len());
        let size = file.read(&mut content[offset..end]).await?;
        offset += size;
    }

    Ok(content)
}

pub(crate) fn random_bytes(size: usize) -> Vec<u8> {
    let mut content = vec![0; size];
    rand::thread_rng().fill(&mut content[..]);
    content
}

fn to_megabytes(bytes: usize) -> usize {
    bytes / 1024 / 1024
}

pub(crate) fn init_log() {
    // log output is captured by default and only shown on failure. Run tests with `--nocapture` to
    // override.
    init_log_with_writer(TestWriter::default())
}

pub(crate) fn init_log_with_writer<W>(writer: W)
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::fmt()
        .event_format(Formatter::<SystemTime>::default())
        .with_writer(writer)
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                // Only show the logs if explicitly enabled with the `RUST_LOG` env variable.
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        .try_init()
        // `Err` here just means the logger is already initialized, it's OK to ignore it.
        .unwrap_or(());
}

#[cfg(feature = "prometheus")]
fn init_recorder(runtime: &Handle) -> metrics_ext::Shared {
    use metrics_ext::Shared;
    Shared::new(init_prometheus_recorder(runtime))
}

#[cfg(feature = "influxdb")]
fn init_recorder(runtime: &Handle) -> metrics_ext::Shared {
    use metrics_ext::Shared;
    Shared::new(init_influxdb_recorder(runtime))
}

#[cfg(all(feature = "prometheus", feature = "influxdb"))]
fn init_recorder(runtime: &Handle, watch_recorder: WatchRecorder) -> metrics_ext::Shared {
    use metrics_ext::{Pair, Shared};

    Shared::new(Pair(
        init_prometheus_recorder(runtime),
        init_influxdb_recorder(runtime),
    ))
}

#[cfg(not(any(feature = "prometheus", feature = "influxdb")))]
fn init_recorder(_runtime: &Handle) -> metrics_ext::Shared {
    use metrics_ext::Shared;
    Shared::new(NoopRecorder)
}

#[cfg(feature = "prometheus")]
fn init_prometheus_recorder(runtime: &Handle) -> impl Recorder {
    use metrics_exporter_prometheus::PrometheusBuilder;

    let endpoint = std::env::var("PROMETHEUS_PUSH_GATEWAY_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:9091/metrics/job/ouisync".to_string());

    let (recorder, exporter) = PrometheusBuilder::new()
        .with_push_gateway(endpoint, Duration::from_millis(100), None, None)
        .unwrap()
        .build()
        .unwrap();

    runtime.spawn(exporter);

    recorder
}

#[cfg(feature = "influxdb")]
fn init_influxdb_recorder(runtime: &Handle) -> impl Recorder {
    use metrics_ext::{InfluxDbParams, InfluxDbRecorder};
    use std::env;

    let params = InfluxDbParams {
        endpoint: env::var("INFLUXDB_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:8086/api/v2".to_string()),
        token: env::var("INFLUXDB_TOKEN").expect("INFLUXDB_TOKEN (InfluxDB API token)"),
        org: env::var("INFLUXDB_ORG").unwrap_or_else(|_| "equalitie".to_string()),
        bucket: env::var("INFLUXDB_BUCKET").unwrap_or_else(|_| "ouisync".to_string()),
    };

    let (recorder, exporter) = InfluxDbRecorder::new(params);

    runtime.spawn(exporter);

    recorder
}

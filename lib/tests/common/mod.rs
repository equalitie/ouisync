mod wait_map;

use camino::Utf8Path;
use comfy_table::{presets, CellAlignment, Table};
use once_cell::sync::Lazy;
use ouisync::{
    crypto::sign::PublicKey,
    metrics::{self, Metrics},
    network::{Network, Registration},
    Access, AccessSecrets, DeviceId, EntryType, Error, Event, File, Payload, PeerAddr, Repository,
    Result, StoreError,
};
use rand::Rng;
use std::{
    fmt,
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};
use tokio::{
    sync::broadcast::{self, error::RecvError},
    task_local,
    time::{self, Duration},
};
use tracing::{instrument, Instrument, Span};

pub(crate) use self::env::*;
use self::wait_map::WaitMap;

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
            let context = Context::new();

            let runtime = runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();

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
            let span = tracing::info_span!("actor", name);

            let f = ACTOR.scope(actor, f);
            let f = f.instrument(span);

            self.tasks.push(self.runtime.spawn(f));
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let result = self
                .runtime
                .block_on(future::try_join_all(self.tasks.drain(..)));

            let (tx, rx) = oneshot::channel();

            self.context.clocks.report(|report| {
                report_metrics(report);
                tx.send(()).ok();
            });

            rx.blocking_recv().ok();

            result.unwrap();
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
            let context = Context::new();
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
            let span = tracing::info_span!("actor", name);

            let f = async move {
                f.await;
                Ok(())
            };
            let f = ACTOR.scope(actor, f);
            let f = f.instrument(span);

            self.runner.client(name, f);
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
    use ouisync::{AccessMode, RepositoryParams, StateMonitor};

    pub(crate) fn create_unbound_network() -> Network {
        Network::new(None, StateMonitor::make_root())
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

    pub(crate) fn get_repo_params_and_secrets(name: &str) -> (RepositoryParams, AccessSecrets) {
        ACTOR.with(|actor| {
            (
                RepositoryParams::new(actor.repo_path(name))
                    .with_device_id(actor.device_id)
                    .with_clocks(actor.context.clocks.clone()),
                actor
                    .context
                    .repo_map
                    .get_or_insert_with(name.to_owned(), AccessSecrets::random_write),
            )
        })
    }

    pub(crate) async fn create_repo_with_mode(name: &str, mode: AccessMode) -> Repository {
        let (params, secrets) = get_repo_params_and_secrets(name);

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

    #[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
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
    clocks: Metrics,
}

impl Context {
    fn new() -> Self {
        init_log();

        Self {
            base_dir: TempDir::new(),
            addr_map: WaitMap::new(),
            repo_map: WaitMap::new(),
            clocks: Metrics::new(),
        }
    }
}

struct Actor {
    name: String,
    context: Arc<Context>,
    base_dir: PathBuf,
    device_id: DeviceId,
}

impl Actor {
    fn new(name: String, context: Arc<Context>) -> Self {
        let base_dir = context.base_dir.path().join(&name);

        Actor {
            name,
            context,
            base_dir,
            device_id: rand::random(),
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
            tracing::warn!("preserving temp dir in '{}'", path.display());
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Proto {
    #[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
    Tcp,
    #[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
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

// Keep calling `f` until it returns `true`. Wait for repo notification between calls.
#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
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

            wait(&mut rx).await
        }
    })
    .await
    .unwrap()
}

pub(crate) async fn wait(rx: &mut broadcast::Receiver<Event>) {
    loop {
        match time::timeout(*EVENT_TIMEOUT, rx.recv()).await {
            Ok(event) => {
                tracing::debug!(?event);

                match event {
                    Ok(Event {
                        payload:
                            Payload::BranchChanged(_)
                            | Payload::BlockReceived { .. }
                            | Payload::MaintenanceCompleted,
                        ..
                    })
                    | Err(RecvError::Lagged(_)) => return,
                    Ok(Event { .. }) => continue,
                    Err(RecvError::Closed) => panic!("notification channel unexpectedly closed"),
                }
            }
            Err(_) => {
                const MESSAGE: &str = "timeout waiting for notification";

                // NOTE: in release mode backtrace is useless so this trace helps us to locate the
                // source of the panic:
                tracing::error!("{}", MESSAGE);
                panic!("{}", MESSAGE);
            }
        }
    }
}

/// Wait until the file at `path` has the expected content. Panics if timeout elapses before the
/// file content matches.
#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
pub(crate) async fn expect_file_content(repo: &Repository, path: &str, expected_content: &[u8]) {
    expect_file_version_content(repo, path, None, expected_content).await
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
            tracing::warn!(path, ?error, "read failed");
            return false;
        }
        Err(error) => {
            tracing::error!(path, ?error);
            panic!("unexpected error: {error:?}");
        }
    };

    if actual_content == expected_content {
        tracing::debug!(path, "content matches");
        true
    } else {
        tracing::warn!(path, "content does not match");
        false
    }
}

pub(crate) async fn open_file_version(
    repo: &Repository,
    path: &str,
    branch_id: Option<&PublicKey>,
) -> Option<File> {
    tracing::debug!(path, "opening");

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
            tracing::warn!(path, ?branch_id, ?error, "open failed");
            return None;
        }
        Err(error) => {
            tracing::error!(path, ?branch_id, ?error);
            panic!("unexpected error: {error:?}");
        }
    };

    tracing::debug!(path, branch.id = ?file.branch().id(), "opened");

    Some(file)
}

#[instrument(skip(repo))]
pub(crate) async fn expect_entry_exists(repo: &Repository, path: &str, entry_type: EntryType) {
    eventually(repo, || check_entry_exists(repo, path, entry_type)).await
}

#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
pub(crate) async fn check_entry_exists(
    repo: &Repository,
    path: &str,
    entry_type: EntryType,
) -> bool {
    tracing::debug!(path, "opening");

    let result = match entry_type {
        EntryType::File => repo.open_file(path).await.map(|_| ()),
        EntryType::Directory => repo.open_directory(path).await.map(|_| ()),
    };

    match result {
        Ok(()) => {
            tracing::debug!(path, "opened");
            true
        }
        Err(
            error @ (Error::EntryNotFound
            | Error::Store(StoreError::BlockNotFound)
            | Error::Store(StoreError::LocatorNotFound)),
        ) => {
            tracing::warn!(path, ?error, "open failed");
            false
        }
        Err(error) => {
            tracing::error!(path, ?error);
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
                tracing::debug!(%path, "still exists");
                false
            }
            Err(Error::EntryNotFound) => true,
            Err(error) => {
                tracing::error!(%path, ?error);
                panic!("unexpected error: {error:?}");
            }
        }
    })
    .await
}

#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
pub(crate) async fn write_in_chunks(file: &mut File, content: &[u8], chunk_size: usize) {
    for offset in (0..content.len()).step_by(chunk_size) {
        let end = (offset + chunk_size).min(content.len());
        file.write_all(&content[offset..end]).await.unwrap();

        if to_megabytes(end) > to_megabytes(offset) {
            tracing::debug!(
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

#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
pub(crate) fn random_content(size: usize) -> Vec<u8> {
    let mut content = vec![0; size];
    rand::thread_rng().fill(&mut content[..]);
    content
}

fn to_megabytes(bytes: usize) -> usize {
    bytes / 1024 / 1024
}

/// Helper to assert two byte slices are equal which prints useful info if they are not.
#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
#[track_caller]
pub(crate) fn assert_content_equal(lhs: &[u8], rhs: &[u8]) {
    let Some(snip_start) = lhs.iter().zip(rhs).position(|(lhs, rhs)| lhs != rhs) else {
        return;
    };

    let snip_len = 32;
    let snip_end = snip_start + snip_len;

    let lhs_snip = &lhs[snip_start..snip_end.min(lhs.len())];
    let rhs_snip = &rhs[snip_start..snip_end.min(rhs.len())];

    let ellipsis_start = if snip_start > 0 { "…" } else { "" };
    let lhs_ellipsis_end = if snip_end < lhs.len() { "…" } else { "" };
    let rhs_ellipsis_end = if snip_end < rhs.len() { "…" } else { "" };

    panic!(
        "content not equal (differing offset: {})\n    lhs: {}{:x}{}\n    rhs: {}{:x}{}",
        snip_start,
        ellipsis_start,
        HexFmt(lhs_snip),
        lhs_ellipsis_end,
        ellipsis_start,
        HexFmt(rhs_snip),
        rhs_ellipsis_end,
    );

    struct HexFmt<'a>(&'a [u8]);

    impl fmt::LowerHex for HexFmt<'_> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            for byte in self.0 {
                write!(f, "{:02x}", byte)?;
            }

            Ok(())
        }
    }
}

pub(crate) fn init_log() {
    use ouisync_tracing_fmt::Formatter;
    use tracing::metadata::LevelFilter;
    use tracing_subscriber::fmt::time::SystemTime;

    let result = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                // Only show the logs if explicitly enabled with the `RUST_LOG` env variable.
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        .event_format(Formatter::<SystemTime>::default())
        // log output is captured by default and only shown on failure. Run tests with
        // `--nocapture` to override.
        .with_test_writer()
        .try_init();

    // error here most likely means the logger is already initialized. We can ignore that.
    result.ok();
}

fn report_metrics(report: &metrics::Report) {
    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(vec![
        "name", "count", "min", "max", "mean", "stdev", "50%", "90%", "99%", "99.9%",
    ]);

    for column in table.column_iter_mut().skip(1) {
        column.set_cell_alignment(CellAlignment::Right);
    }

    for item in report.items() {
        let v = item.time;

        table.add_row(vec![
            format!("{}", item.name),
            format!("{}", v.count()),
            format!("{:.4}", v.min().as_secs_f64()),
            format!("{:.4}", v.max().as_secs_f64()),
            format!("{:.4}", v.mean().as_secs_f64()),
            format!("{:.4}", v.stdev().as_secs_f64()),
            format!("{:.4}", v.value_at_quantile(0.5).as_secs_f64()),
            format!("{:.4}", v.value_at_quantile(0.9).as_secs_f64()),
            format!("{:.4}", v.value_at_quantile(0.99).as_secs_f64()),
            format!("{:.4}", v.value_at_quantile(0.999).as_secs_f64()),
        ]);
    }

    println!("Time (s)\n{table}");

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(vec![
        "name", "count", "min", "max", "mean", "stdev", "50%", "90%", "99%", "99.9%",
    ]);

    for column in table.column_iter_mut().skip(1) {
        column.set_cell_alignment(CellAlignment::Right);
    }

    for item in report.items() {
        let v = item.throughput;

        table.add_row(vec![
            format!("{}", item.name),
            format!("{}", v.count()),
            format!("{:.1}", v.min()),
            format!("{:.1}", v.max()),
            format!("{:.1}", v.mean()),
            format!("{:.1}", v.stdev()),
            format!("{:.1}", v.value_at_quantile(0.5)),
            format!("{:.1}", v.value_at_quantile(0.9)),
            format!("{:.1}", v.value_at_quantile(0.99)),
            format!("{:.1}", v.value_at_quantile(0.999)),
        ]);
    }

    println!("Throughput (hits/s)\n{table}");
}

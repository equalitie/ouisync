use camino::Utf8Path;
use ouisync::{
    crypto::sign::PublicKey,
    network::{Network, Registration},
    Access, AccessSecrets, ConfigStore, DeviceId, EntryType, Error, Event, File, Payload, PeerAddr,
    Repository, RepositoryDb, Result,
};
use rand::Rng;
use std::{
    cell::Cell,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    thread,
};
use tokio::{
    sync::broadcast::{self, error::RecvError},
    task_local,
    time::{self, Duration},
};
use tracing::{instrument, Instrument, Span};

pub(crate) use self::env::*;

// Timeout for running a whole test case
pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(120);

// Timeout for waiting for an event
pub(crate) const EVENT_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(not(feature = "simulation"))]
pub(crate) mod env {
    use super::*;
    use futures_util::future;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use tokio::{
        runtime::{self, Runtime},
        task::JoinHandle,
    };

    /// Test environment that uses real network (localhost)
    pub(crate) struct Env {
        context: Context,
        runtime: Runtime,
        tasks: Vec<JoinHandle<()>>,
        default_port: u16,
        dns: Arc<Mutex<Dns>>,
    }

    impl Env {
        pub fn new() -> Self {
            let context = Context::new();

            let runtime = runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();

            Self {
                context,
                runtime,
                tasks: Vec::new(),
                default_port: next_default_port(),
                dns: Arc::new(Mutex::new(Dns::new())),
            }
        }

        pub fn actor<Fut>(&mut self, name: &str, f: Fut)
        where
            Fut: Future<Output = ()> + Send + 'static,
        {
            let actor = Actor::new(self.context.base_dir.path().join(name));
            let span = tracing::info_span!("actor", name);

            self.dns.lock().unwrap().register(name);

            let f = DNS.scope(self.dns.clone(), f);
            let f = DEFAULT_PORT.scope(self.default_port, f);
            let f = DEFAULT_SECRETS.scope(self.context.default_secrets.clone(), f);
            let f = ACTOR.scope(actor, f);
            let f = ACTOR_NAME.scope(name.to_owned(), f);
            let f = f.instrument(span);

            self.tasks.push(self.runtime.spawn(f));
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            self.runtime
                .block_on(future::try_join_all(self.tasks.drain(..)))
                .unwrap();
        }
    }

    pub(super) fn bind_addr() -> IpAddr {
        let name = ACTOR_NAME.with(|name| name.clone());
        lookup(&name)
    }

    pub(super) fn lookup(target: &str) -> IpAddr {
        DNS.with(|dns| dns.lock().unwrap().lookup(target))
    }

    pub(super) fn default_port() -> u16 {
        DEFAULT_PORT.with(|port| *port)
    }

    fn next_default_port() -> u16 {
        use std::sync::atomic::{AtomicU16, Ordering};

        static NEXT: AtomicU16 = AtomicU16::new(7000);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    struct Dns {
        next_last_octet: u8,
        addrs: HashMap<String, IpAddr>,
    }

    impl Dns {
        fn new() -> Self {
            Self {
                next_last_octet: 1,
                addrs: HashMap::default(),
            }
        }

        fn register(&mut self, name: &str) -> IpAddr {
            let last_octet = self.next_last_octet;
            self.next_last_octet = self
                .next_last_octet
                .checked_add(1)
                .expect("too many dns records");

            let addr = Ipv4Addr::new(127, 0, 0, last_octet).into();

            assert!(
                self.addrs.insert(name.to_owned(), addr).is_none(),
                "dns record already exists"
            );

            addr
        }

        fn lookup(&self, name: &str) -> IpAddr {
            self.addrs.get(name).copied().unwrap()
        }
    }

    task_local! {
        static DNS: Arc<Mutex<Dns>>;
        static DEFAULT_PORT: u16;
        static ACTOR_NAME: String;
    }
}

#[cfg(feature = "simulation")]
pub(crate) mod env {
    use super::*;

    /// Test environment that uses simulated network
    pub(crate) struct Env<'a> {
        context: Context,
        runner: turmoil::Sim<'a>,
    }

    impl<'a> Env<'a> {
        pub fn new() -> Self {
            let context = Context::new();
            let runner = turmoil::Builder::new()
                .simulation_duration(Duration::from_secs(90))
                .build_with_rng(Box::new(rand::thread_rng()));

            Self { context, runner }
        }

        pub fn actor<Fut>(&mut self, name: &str, f: Fut)
        where
            Fut: Future<Output = ()> + 'static,
        {
            let actor = Actor::new(self.context.base_dir.path().join(name));
            let span = tracing::info_span!("actor", name);

            let f = async move {
                f.await;
                Ok(())
            };
            let f = ACTOR.scope(actor, f);
            let f = DEFAULT_SECRETS.scope(self.context.default_secrets.clone(), f);
            let f = f.instrument(span);

            self.runner.client(name, f);
        }
    }

    impl Drop for Env<'_> {
        fn drop(&mut self) {
            self.runner.run().unwrap()
        }
    }

    pub(super) fn bind_addr() -> IpAddr {
        Ipv4Addr::UNSPECIFIED.into()
    }

    pub(super) fn lookup(target: &str) -> IpAddr {
        turmoil::lookup(target)
    }

    pub(super) const fn default_port() -> u16 {
        // The simulated network is isolated from other tests that might be running in parallel so
        // using a constant port is ok as it can't clash with those other tests.
        7000
    }
}

/// Operations on the current actor. All of these function can only be called in an actor context,
/// that is, from inside the future passed to `Env::actor`.
pub(crate) mod actor {
    use super::*;

    pub(crate) fn create_unbound_network() -> Network {
        let config_store = ACTOR.with(|actor| actor.base_dir.join("config"));
        let config_store = ConfigStore::new(config_store);

        Network::new(config_store)
    }

    pub(crate) async fn create_network(proto: Proto) -> Network {
        let network = create_unbound_network();

        let bind_addr = SocketAddr::new(env::bind_addr(), env::default_port());
        let bind_addr = proto.wrap(bind_addr);
        network.handle().bind(&[bind_addr]).await;

        network
    }

    pub(crate) fn make_repo_path() -> PathBuf {
        ACTOR.with(|actor| actor.next_repo_path())
    }

    pub(crate) fn device_id() -> DeviceId {
        ACTOR.with(|actor| actor.device_id)
    }

    pub(crate) fn default_secrets() -> AccessSecrets {
        DEFAULT_SECRETS.with(|secrets| secrets.clone())
    }

    pub(crate) async fn create_repo_with_secrets(secrets: AccessSecrets) -> Repository {
        Repository::create(
            RepositoryDb::create(&make_repo_path()).await.unwrap(),
            device_id(),
            Access::new(None, None, secrets),
        )
        .await
        .unwrap()
    }

    pub(crate) async fn create_repo() -> Repository {
        create_repo_with_secrets(default_secrets()).await
    }

    pub(crate) async fn create_linked_repo(network: &Network) -> (Repository, Registration) {
        let repo = create_repo().await;
        let reg = network.handle().register(repo.store().clone());

        (repo, reg)
    }

    #[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
    /// Convenience function for the common case where the actor has one linked repository.
    pub(crate) async fn setup() -> (Network, Repository, Registration) {
        let network = create_network(Proto::Tcp).await;
        let (repo, reg) = create_linked_repo(&network).await;
        (network, repo, reg)
    }
}

task_local! {
    static ACTOR: Actor;
    static DEFAULT_SECRETS: AccessSecrets;
}

struct Context {
    base_dir: TempDir,
    default_secrets: AccessSecrets,
}

impl Context {
    fn new() -> Self {
        init_log();

        Self {
            base_dir: TempDir::new(),
            default_secrets: AccessSecrets::random_write(),
        }
    }
}

struct Actor {
    base_dir: PathBuf,
    device_id: DeviceId,
    repo_counter: Cell<u32>,
}

impl Actor {
    fn new(base_dir: PathBuf) -> Self {
        Actor {
            base_dir,
            device_id: rand::random(),
            repo_counter: Cell::new(0),
        }
    }

    fn next_repo_path(&self) -> PathBuf {
        let num = self.repo_counter.get();
        self.repo_counter.set(num + 1);

        self.base_dir.join(format!("repo-{num}.db"))
    }
}

pub(crate) trait NetworkExt {
    fn connect(&self, to: &str);
    fn knows(&self, to: &str) -> bool;
}

impl NetworkExt for Network {
    fn connect(&self, to: &str) {
        self.add_user_provided_peer(&peer_addr(self, to))
    }

    fn knows(&self, to: &str) -> bool {
        self.knows_peer(peer_addr(self, to))
    }
}

fn peer_addr(network: &Network, name: &str) -> PeerAddr {
    Proto::detect(network).wrap(SocketAddr::new(env::lookup(name), env::default_port()))
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

#[derive(Clone, Copy)]
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

    #[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
    #[track_caller]
    pub fn listener_local_addr_v4(&self, network: &Network) -> PeerAddr {
        match self {
            Self::Tcp => PeerAddr::Tcp(network.tcp_listener_local_addr_v4().unwrap()),
            Self::Quic => PeerAddr::Quic(network.quic_listener_local_addr_v4().unwrap()),
        }
    }

    #[track_caller]
    pub fn detect(network: &Network) -> Self {
        match (
            network.quic_listener_local_addr_v4(),
            network.quic_listener_local_addr_v6(),
            network.tcp_listener_local_addr_v4(),
            network.tcp_listener_local_addr_v6(),
        ) {
            (Some(_), _, _, _) | (_, Some(_), _, _) => Self::Quic,
            (_, _, Some(_), _) | (_, _, _, Some(_)) => Self::Tcp,
            (None, None, None, None) => panic!("no protocol"),
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

    time::timeout(TEST_TIMEOUT, async {
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
        match time::timeout(EVENT_TIMEOUT, rx.recv()).await {
            Ok(Ok(Event {
                payload: Payload::BranchChanged(_) | Payload::BlockReceived { .. },
                ..
            }))
            | Ok(Err(RecvError::Lagged(_))) => return,
            Ok(Ok(Event { .. })) => continue,
            Ok(Err(RecvError::Closed)) => panic!("notification channel unexpectedly closed"),
            Err(_) => panic!("timeout waiting for notification"),
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
    tracing::debug!(path, "opening");

    let result = if let Some(branch_id) = branch_id {
        repo.open_file_version(path, branch_id).await
    } else {
        repo.open_file(path).await
    };

    let mut file = match result {
        Ok(file) => file,
        // `EntryNotFound` likely means that the parent directory hasn't yet been fully synced
        // and so the file entry is not in it yet.
        //
        // `BlockNotFound` means the first block of the file hasn't been downloaded yet.
        Err(error @ (Error::EntryNotFound | Error::BlockNotFound(_))) => {
            tracing::warn!(path, ?error, "open failed");
            return false;
        }
        Err(error) => panic!("unexpected error: {error:?}"),
    };

    tracing::debug!(path, branch.id = ?file.branch().id(), "opened");

    let actual_content = match read_in_chunks(&mut file, 4096).await {
        Ok(content) => content,
        // `BlockNotFound` means just the some block of the file hasn't been downloaded yet.
        Err(error @ Error::BlockNotFound(_)) => {
            tracing::warn!(path, ?error, "read failed");
            return false;
        }
        Err(error) => panic!("unexpected error: {error:?}"),
    };

    if actual_content == expected_content {
        tracing::debug!(path, "content matches");
        true
    } else {
        tracing::warn!(path, "content does not match");
        false
    }
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
    let result = match entry_type {
        EntryType::File => repo.open_file(path).await.map(|_| ()),
        EntryType::Directory => repo.open_directory(path).await.map(|_| ()),
    };

    match result {
        Ok(()) => true,
        Err(Error::EntryNotFound | Error::BlockNotFound(_)) => false,
        Err(error) => panic!("unexpected error: {error:?}"),
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
            Ok(_) => false,
            Err(Error::EntryNotFound) => true,
            Err(error) => panic!("unexpected error: {error:?}"),
        }
    })
    .await
}

#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
pub(crate) async fn write_in_chunks(file: &mut File, content: &[u8], chunk_size: usize) {
    for offset in (0..content.len()).step_by(chunk_size) {
        let end = (offset + chunk_size).min(content.len());
        file.write(&content[offset..end]).await.unwrap();

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

pub(crate) fn init_log() {
    use tracing::metadata::LevelFilter;

    let builder = tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                // Only show the logs if explicitly enabled with the `RUST_LOG` env variable.
                .with_default_directive(LevelFilter::OFF.into())
                .from_env_lossy(),
        )
        // log output is captured by default and only shown on failure. Run tests with
        // `--nocapture` to override.
        .with_test_writer();

    let result = if cfg!(feature = "simulation") {
        // Disable printing time so that logs from the same seed are identical
        builder.without_time().try_init()
    } else {
        builder.try_init()
    };

    // error here most likely means the logger is already initialized. We can ignore that.
    result.ok();
}

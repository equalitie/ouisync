use ouisync::{
    network::{Network, NetworkOptions},
    AccessSecrets, BranchChangedReceiver, ConfigStore, MasterSecret, PeerAddr, Repository,
    StateMonitor,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    thread,
};
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;

// Test environment
pub(crate) struct Env {
    pub rng: StdRng,
    base_dir: TempDir,
    next_repo_num: u64,
    _span: tracing::span::EnteredSpan,
}

impl Env {
    pub fn with_seed(seed: u64) -> Self {
        Self::with_rng(StdRng::seed_from_u64(seed))
    }

    pub fn with_rng(rng: StdRng) -> Self {
        init_log();

        let span = tracing::info_span!("test", name = thread::current().name()).entered();
        let base_dir = TempDir::new().unwrap();

        Self {
            rng,
            base_dir,
            next_repo_num: 0,
            _span: span,
        }
    }

    pub fn next_store(&mut self) -> PathBuf {
        let num = self.next_repo_num;
        self.next_repo_num += 1;

        self.base_dir.path().join(format!("repo-{}.db", num))
    }

    pub async fn create_repo(&mut self) -> Repository {
        let secrets = AccessSecrets::generate_write(&mut self.rng);
        self.create_repo_with_secrets(secrets).await
    }

    pub async fn create_repo_with_secrets(&mut self, secrets: AccessSecrets) -> Repository {
        Repository::create(
            &self.next_store(),
            self.rng.gen(),
            MasterSecret::generate(&mut self.rng),
            secrets,
            true,
            &StateMonitor::make_root(),
        )
        .await
        .unwrap()
    }

    pub async fn create_linked_repos(&mut self) -> (Repository, Repository) {
        let repo_a = self.create_repo().await;
        let repo_b = self
            .create_repo_with_secrets(repo_a.secrets().clone())
            .await;

        (repo_a, repo_b)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Proto {
    Tcp,
    Quic,
}

impl Proto {
    pub fn of(addr: &PeerAddr) -> Self {
        match addr {
            PeerAddr::Tcp(_) => Self::Tcp,
            PeerAddr::Quic(_) => Self::Quic,
        }
    }

    pub fn wrap_addr(&self, addr: SocketAddr) -> PeerAddr {
        match self {
            Self::Tcp => PeerAddr::Tcp(addr),
            Self::Quic => PeerAddr::Quic(addr),
        }
    }

    pub fn listener_local_addr_v4(&self, network: &Network) -> PeerAddr {
        match self {
            Self::Tcp => PeerAddr::Tcp(*network.tcp_listener_local_addr_v4().unwrap()),
            Self::Quic => PeerAddr::Quic(*network.quic_listener_local_addr_v4().unwrap()),
        }
    }
}

// Create two `Network` instances connected together.
pub(crate) async fn create_connected_peers(proto: Proto) -> (Network, Network) {
    let a = Network::new(
        &test_network_options(proto),
        ConfigStore::null(),
        StateMonitor::make_root(),
    )
    .await
    .unwrap();

    let b = create_peer_connected_to(proto.listener_local_addr_v4(&a)).await;

    (a, b)
}

// Create a `Network` instance connected only to the given address.
pub(crate) async fn create_peer_connected_to(addr: PeerAddr) -> Network {
    Network::new(
        &NetworkOptions {
            peers: vec![addr],
            ..test_network_options(Proto::of(&addr))
        },
        ConfigStore::null(),
        StateMonitor::make_root(),
    )
    .await
    .unwrap()
}

pub(crate) fn test_network_options(proto: Proto) -> NetworkOptions {
    NetworkOptions {
        bind: vec![proto.wrap_addr((Ipv4Addr::LOCALHOST, 0).into())],
        disable_local_discovery: true,
        disable_upnp: true,
        disable_dht: true,
        ..Default::default()
    }
}

// Keep calling `f` until it returns `true`. Wait for repo notification between calls.
pub(crate) async fn eventually<F, Fut>(repo: &Repository, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let mut rx = repo.subscribe();

    loop {
        if f().await {
            break;
        }

        wait(&mut rx).await;
    }
}

pub(crate) async fn wait(rx: &mut BranchChangedReceiver) {
    match rx.recv().await {
        Ok(_) | Err(RecvError::Lagged(_)) => (),
        Err(RecvError::Closed) => panic!("notification channel unexpectedly closed"),
    }
}

fn init_log() {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        // error here most likely means the logger is already initialized. We can ignore that.
        .ok();
}

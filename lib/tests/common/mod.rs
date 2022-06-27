use ouisync::{
    db,
    network::{Network, NetworkOptions},
    AccessSecrets, ConfigStore, MasterSecret, PeerAddr, Repository,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
};
use tempfile::TempDir;

// Test environment
pub(crate) struct Env {
    pub rng: StdRng,
    base_dir: TempDir,
    next_repo_num: u64,
}

impl Env {
    pub fn with_seed(seed: u64) -> Self {
        Self::with_rng(StdRng::seed_from_u64(seed))
    }

    pub fn with_rng(rng: StdRng) -> Self {
        let base_dir = TempDir::new().unwrap();
        Self {
            rng,
            base_dir,
            next_repo_num: 0,
        }
    }

    pub fn next_store(&mut self) -> db::Store {
        let num = self.next_repo_num;
        self.next_repo_num += 1;

        let path = self.base_dir.path().join(format!("repo-{}.db", num));

        db::Store::Permanent(path)
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

// Create two `Network` instances connected together.
pub(crate) async fn create_connected_peers() -> (Network, Network) {
    let a = Network::new(&test_network_options(), ConfigStore::null())
        .await
        .unwrap();

    let b = create_peer_connected_to(*a.tcp_listener_local_addr_v4().unwrap()).await;

    (a, b)
}

// Create a `Network` instance connected only to the given address.
pub(crate) async fn create_peer_connected_to(addr: SocketAddr) -> Network {
    Network::new(
        &NetworkOptions {
            peers: vec![PeerAddr::Tcp(addr)],
            ..test_network_options()
        },
        ConfigStore::null(),
    )
    .await
    .unwrap()
}

pub(crate) fn test_network_options() -> NetworkOptions {
    NetworkOptions {
        bind: vec![PeerAddr::Tcp((Ipv4Addr::LOCALHOST, 0).into())],
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

        rx.recv().await.unwrap();
    }
}

use camino::Utf8Path;
use ouisync::{
    network::{Network, Registration},
    Access, Event, Payload, PeerAddr, Repository, RepositoryParams, WriteSecrets,
};
use rand::{rngs::StdRng, Rng, SeedableRng};
use state_monitor::StateMonitor;
use std::{net::Ipv4Addr, ops::Deref, path::Path, time::Duration};
use tokio::{
    runtime::Handle,
    sync::broadcast::{self, error::RecvError},
    time,
};

#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
const EVENT_TIMEOUT: Duration = Duration::from_secs(60);

/// `id` is used to generate the access secrets - repos with the same id have the same secrets.
pub async fn create_repo(
    rng: &mut StdRng,
    store: &Path,
    id: u64,
    monitor: StateMonitor,
) -> RepositoryGuard {
    let mut secret_rng = StdRng::seed_from_u64(id);
    let secrets = WriteSecrets::generate(&mut secret_rng);

    let repository = Repository::create(
        &RepositoryParams::new(store)
            .with_device_id(rng.gen())
            .with_parent_monitor(monitor),
        Access::WriteUnlocked { secrets },
    )
    .await
    .unwrap();

    RepositoryGuard {
        repository,
        handle: Handle::current(),
    }
}

// Wrapper for `Repository` which calls `close` on drop.
pub struct RepositoryGuard {
    repository: Repository,
    handle: Handle,
}

impl Deref for RepositoryGuard {
    type Target = Repository;

    fn deref(&self) -> &Self::Target {
        &self.repository
    }
}

impl Drop for RepositoryGuard {
    fn drop(&mut self) {
        self.handle
            .block_on(async { self.repository.close().await.unwrap() })
    }
}

/// Write `size` random bytes to a file at `path` (`buffer_size` bytes at a time).
pub async fn write_file(
    rng: &mut StdRng,
    repo: &Repository,
    path: &Utf8Path,
    size: usize,
    buffer_size: usize,
    print_progress: bool,
) {
    let mut file = repo.create_file(path).await.unwrap();

    if size == 0 {
        return;
    }

    let mut remaining = size;
    let mut buffer = vec![0; buffer_size];

    while remaining > 0 {
        let len = buffer_size.min(remaining);

        rng.fill(&mut buffer[..len]);
        file.write_all(&buffer[..len]).await.unwrap();

        remaining -= len;

        if print_progress {
            println!("{:.1}%", 100.0 * (size - remaining) as f64 / size as f64);
        }
    }

    file.flush().await.unwrap();
}

/// Read the whole content of the file at `path` in `buffer_size` bytes at a time. Returns the
/// total size of the content.
pub async fn read_file(repo: &Repository, path: &Utf8Path, buffer_size: usize) -> usize {
    let mut file = repo.open_file(path).await.unwrap();
    let mut buffer = vec![0; buffer_size];
    let mut size = 0;

    loop {
        let len = file.read(&mut buffer[..]).await.unwrap();

        if len == 0 {
            break;
        }

        size += len;
    }

    size
}

#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
pub(crate) struct Actor {
    pub network: Network,
    pub repo: RepositoryGuard,
    pub _reg: Registration,
}

impl Actor {
    #[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
    pub(crate) async fn new(rng: &mut StdRng, base_dir: &Path) -> Self {
        let monitor = StateMonitor::make_root();

        let network = Network::new(monitor.clone(), None, None);
        network
            .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;

        let repo = create_repo(rng, &base_dir.join("repo.db"), 0, monitor).await;
        let reg = network.register(repo.handle()).await;

        Self {
            network,
            repo,
            _reg: reg,
        }
    }

    #[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
    pub(crate) fn connect_to(&self, peer: &Actor) {
        let addr = peer
            .network
            .listener_local_addrs()
            .into_iter()
            .next()
            .unwrap();

        self.network.add_user_provided_peer(&addr);
    }
}

#[allow(unused)] // https://github.com/rust-lang/rust/issues/46379
pub(crate) async fn wait_for_sync(repo_a: &Repository, repo_b: &Repository) {
    let mut rx = repo_a.subscribe();

    loop {
        let vv_a = repo_a
            .local_branch()
            .unwrap()
            .version_vector()
            .await
            .unwrap();
        let vv_b = repo_b
            .local_branch()
            .unwrap()
            .version_vector()
            .await
            .unwrap();

        let progress_a = repo_a.sync_progress().await.unwrap();

        if progress_a.value == progress_a.total && vv_a >= vv_b {
            break;
        }

        wait_for_event(&mut rx).await;
    }
}

async fn wait_for_event(rx: &mut broadcast::Receiver<Event>) {
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

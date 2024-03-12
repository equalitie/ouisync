use crate::{
    file::FileHolder,
    mounter::Mounter,
    registry::{Handle, SharedRegistry},
    repository::Repositories,
};
use ouisync_bridge::{config::ConfigStore, transport};
use ouisync_lib::network::Network;
use scoped_task::ScopedJoinHandle;
use state_monitor::StateMonitor;
use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{oneshot, OnceCell};

pub(crate) struct State {
    pub config: ConfigStore,
    pub files: SharedRegistry<Arc<FileHolder>>,
    pub mounter: Mounter,
    pub network: Network,
    pub remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
    pub repositories: Repositories,
    pub repos_monitor: StateMonitor,
    pub root_monitor: StateMonitor,
    tasks: SharedRegistry<ScopedJoinHandle<()>>,
}

impl State {
    pub fn new(configs_path: PathBuf, root_monitor: StateMonitor) -> Self {
        let config = ConfigStore::new(configs_path);

        let network = Network::new(
            Some(config.dht_contacts_store()),
            root_monitor.make_child("Network"),
        );

        let repos_monitor = root_monitor.make_child("Repositories");

        Self {
            config,
            files: SharedRegistry::new(),
            mounter: Mounter::new(),
            network,
            remote_client_config: OnceCell::new(),
            repositories: Repositories::new(),
            repos_monitor,
            root_monitor,
            tasks: SharedRegistry::new(),
        }
    }

    pub async fn get_remote_client_config(&self) -> io::Result<Arc<rustls::ClientConfig>> {
        self.remote_client_config
            .get_or_try_init(|| make_remote_client_config(self.config.dir()))
            .await
            .cloned()
    }

    /// Spawns a task and inserts it into the `tasks` registry. Returns its Registry handle.
    pub fn spawn_task<M, F>(&self, make_task: M) -> TaskHandle
    where
        M: FnOnce(u64) -> F + Send + 'static,
        F: Future<Output = ()> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        let task = scoped_task::spawn(async move {
            let Ok(id) = rx.await else {
                return;
            };

            make_task(id).await
        });

        let handle = self.tasks.insert(task);

        tx.send(handle.id()).ok();

        handle
    }

    /// Cancel a notification subscription.
    pub fn remove_task(&self, handle: TaskHandle) {
        self.tasks.remove(handle);
    }
}

pub(crate) type TaskHandle = Handle<ScopedJoinHandle<()>>;

async fn make_remote_client_config(config_dir: &Path) -> io::Result<Arc<rustls::ClientConfig>> {
    // Load custom root certificates (if any)
    let additional_root_certs =
        transport::tls::load_certificates_from_dir(&config_dir.join("root_certs")).await?;
    transport::make_client_config(&additional_root_certs)
}

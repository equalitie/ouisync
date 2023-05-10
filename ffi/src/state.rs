use crate::{
    file::FileHolder,
    registry::{Handle, Registry},
    repository::RepositoryHolder,
};
use once_cell::sync::OnceCell;
use ouisync_bridge::{config::ConfigStore, error::Result, transport::ClientConfig};
use ouisync_lib::{network::Network, StateMonitor};
use scoped_task::ScopedJoinHandle;
use std::path::PathBuf;

pub(crate) struct State {
    pub root_monitor: StateMonitor,
    pub repos_monitor: StateMonitor,
    pub config: ConfigStore,
    pub network: Network,
    pub repositories: Registry<RepositoryHolder>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
    pub remote_client_config: OnceCell<ClientConfig>,
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
            root_monitor,
            repos_monitor,
            config,
            network,
            repositories: Registry::new(),
            files: Registry::new(),
            tasks: Registry::new(),
            remote_client_config: OnceCell::new(),
        }
    }

    /// Cancel a notification subscription.
    pub fn unsubscribe(&self, handle: SubscriptionHandle) {
        self.tasks.remove(handle);
    }

    pub fn get_remote_client_config(&self) -> Result<ClientConfig> {
        self.remote_client_config
            .get_or_try_init(|| ClientConfig::new(&[]))
            .cloned()
    }
}

pub(crate) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;

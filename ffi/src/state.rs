use crate::{
    file::FileHolder,
    registry::{Handle, Registry},
};
use ouisync_bridge::{config::ConfigStore, repository::RepositoryHolder};
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
}

impl State {
    pub fn new(configs_path: PathBuf, root_monitor: StateMonitor) -> Self {
        let config = ConfigStore::new(configs_path);
        let network = Network::new(root_monitor.make_child("Network"));
        let repos_monitor = root_monitor.make_child("Repositories");

        Self {
            root_monitor,
            repos_monitor,
            config,
            network,
            repositories: Registry::new(),
            files: Registry::new(),
            tasks: Registry::new(),
        }
    }

    /// Cancel a notification subscription.
    pub fn unsubscribe(&self, handle: SubscriptionHandle) {
        self.tasks.remove(handle);
    }
}

pub(crate) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;

use crate::{
    file::FileHolder,
    registry::{Handle, Registry},
};
use ouisync_bridge::{config::ConfigStore, repository::RepositoryHolder};
use ouisync_lib::{network::Network, StateMonitor};
use scoped_task::ScopedJoinHandle;
use std::path::PathBuf;
use tracing::Span;

pub(crate) struct State {
    pub root_monitor: StateMonitor,
    pub repos_span: Span,
    pub config: ConfigStore,
    pub network: Network,
    pub repositories: Registry<RepositoryHolder>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
}

impl State {
    pub fn new(configs_path: PathBuf, root_monitor: StateMonitor) -> Self {
        let config = ConfigStore::new(configs_path);

        // Create network
        let network = {
            let _enter = tracing::info_span!("Network").entered();
            Network::new()
        };

        let repos_span = tracing::info_span!("Repositories");

        Self {
            root_monitor,
            repos_span,
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

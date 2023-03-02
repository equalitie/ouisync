use crate::{
    file::FileHolder,
    protocol::Notification,
    registry::{Handle, Registry},
    repository::RepositoryHolder,
};
use camino::Utf8Path;
use ouisync_lib::{network::Network, ConfigStore, StateMonitor};
use scoped_task::ScopedJoinHandle;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::Span;

pub struct ServerState {
    pub(crate) root_monitor: StateMonitor,
    pub(crate) repos_span: Span,
    pub(crate) config: ConfigStore,
    pub network: Network,
    pub repositories: Registry<RepositoryHolder>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
}

impl ServerState {
    pub fn new(configs_path: PathBuf, root_monitor: StateMonitor) -> Self {
        let config = ConfigStore::new(configs_path);

        // Create network
        let network = {
            let _enter = tracing::info_span!("Network").entered();
            Network::new(config.clone())
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

    pub(crate) fn repo_span(&self, store: &Utf8Path) -> Span {
        tracing::info_span!(parent: &self.repos_span, "repo", ?store)
    }
}

pub struct ClientState {
    pub(crate) notification_tx: mpsc::Sender<(u64, Notification)>,
}

pub(crate) type SubscriptionHandle = Handle<ScopedJoinHandle<()>>;

/// Cancel a notification subscription.
pub(crate) fn unsubscribe(state: &ServerState, handle: SubscriptionHandle) {
    state.tasks.remove(handle);
}

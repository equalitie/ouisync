use crate::{
    file::FileHolder, protocol::Notification, registry::Registry, repository::RepositoryHolder,
};
use camino::Utf8Path;
use ouisync_lib::{network::Network, ConfigStore, StateMonitor};
use scoped_task::ScopedJoinHandle;
use tokio::sync::mpsc;
use tracing::Span;

pub(crate) struct ServerState {
    pub root_monitor: StateMonitor,
    pub repos_span: Span,
    pub config: ConfigStore,
    pub network: Network,
    pub repositories: Registry<RepositoryHolder>,
    pub files: Registry<FileHolder>,
    pub tasks: Registry<ScopedJoinHandle<()>>,
}

impl ServerState {
    pub fn repo_span(&self, store: &Utf8Path) -> Span {
        tracing::info_span!(parent: &self.repos_span, "repo", ?store)
    }
}

pub(crate) struct ClientState {
    pub notification_tx: mpsc::Sender<(u64, Notification)>,
}

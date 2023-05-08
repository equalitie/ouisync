use crate::{
    options::Dirs,
    repository::{self, RepositoryMap},
    server::ServerContainer,
};
use camino::Utf8PathBuf;
use futures_util::future;
use ouisync_bridge::{
    config::ConfigStore,
    network::{self, NetworkDefaults},
};
use ouisync_lib::{network::Network, StateMonitor};
use std::{sync::Arc, time::Duration};
use tokio::time;

pub(crate) struct State {
    pub config: ConfigStore,
    pub store_dir: Utf8PathBuf,
    pub mount_dir: Utf8PathBuf,
    pub network: Network,
    pub repositories: RepositoryMap,
    pub repositories_monitor: StateMonitor,
    pub servers: ServerContainer,
}

impl State {
    pub async fn init(dirs: &Dirs, monitor: StateMonitor) -> Arc<Self> {
        let config = ConfigStore::new(&dirs.config_dir);

        let network = Network::new(
            Some(config.dht_contacts_store()),
            monitor.make_child("Network"),
        );

        network::init(
            &network,
            &config,
            NetworkDefaults {
                port_forwarding_enabled: false,
                local_discovery_enabled: false,
            },
        )
        .await;

        let repositories_monitor = monitor.make_child("Repositories");
        let repositories =
            repository::find_all(dirs, &network, &config, &repositories_monitor).await;

        let state = Self {
            config,
            store_dir: dirs.store_dir.clone(),
            mount_dir: dirs.mount_dir.clone(),
            network,
            repositories,
            repositories_monitor,
            servers: ServerContainer::new(),
        };
        let state = Arc::new(state);

        state.servers.init(state.clone()).await;

        state
    }

    pub async fn close(&self) {
        // Kill remote servers
        self.servers.close();

        // Close repos
        let close_repositories = future::join_all(self.repositories.remove_all().into_iter().map(
            |holder| async move {
                if let Err(error) = holder.repository.close().await {
                    tracing::error!(
                        name = %holder.name(),
                        ?error,
                        "failed to gracefully close repository"
                    );
                }
            },
        ));

        let shutdown_network = async move {
            time::timeout(Duration::from_secs(1), self.network.shutdown())
                .await
                .ok();
        };

        future::join(close_repositories, shutdown_network).await;
    }

    pub fn store_path(&self, name: &str) -> Utf8PathBuf {
        repository::store_path(&self.store_dir, name)
    }
}

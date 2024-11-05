use crate::{
    metrics::MetricsServer,
    options::Dirs,
    protocol::Error,
    repository::{self, RepositoryMap},
    server::ServerContainer,
};
use futures_util::future;
use ouisync_bridge::{
    config::ConfigStore,
    network::{self, NetworkDefaults},
    transport,
};
use ouisync_lib::Network;
use state_monitor::StateMonitor;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::{sync::OnceCell, task, time};

pub(crate) struct State {
    pub config: ConfigStore,
    pub store_dir: PathBuf,
    pub mount_dir: PathBuf,
    pub network: Network,
    pub repositories: RepositoryMap,
    pub repositories_monitor: StateMonitor,
    pub rpc_servers: ServerContainer,
    pub metrics_server: MetricsServer,
    pub server_config: OnceCell<Arc<rustls::ServerConfig>>,
    pub client_config: OnceCell<Arc<rustls::ClientConfig>>,
}

impl State {
    pub async fn init(dirs: &Dirs, monitor: StateMonitor) -> Result<Arc<Self>, Error> {
        let config = ConfigStore::new(&dirs.config_dir);

        let network = Network::new(
            monitor.make_child("Network"),
            Some(config.dht_contacts_store()),
            None,
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
            rpc_servers: ServerContainer::new(),
            metrics_server: MetricsServer::new(),
            server_config: OnceCell::new(),
            client_config: OnceCell::new(),
        };
        let state = Arc::new(state);

        state.rpc_servers.init(state.clone()).await?;
        state.metrics_server.init(&state).await?;

        Ok(state)
    }

    /// Starts task to periodically delete expired repositories.
    pub fn delete_expired_repositories(self: Arc<Self>, poll_interval: Duration) -> Arc<Self> {
        task::spawn(delete_expired_repositories(
            Arc::downgrade(&self),
            poll_interval,
        ));

        self
    }

    pub async fn close(&self) {
        // Kill RPC servers
        self.rpc_servers.close();

        // Kill metrics server
        self.metrics_server.close();

        // Close repos
        let close_repositories = future::join_all(
            self.repositories
                .remove_all()
                .into_iter()
                .map(|holder| async move { holder.close().await }),
        );

        let shutdown_network = async move {
            time::timeout(Duration::from_secs(1), self.network.shutdown())
                .await
                .ok();
        };

        future::join(close_repositories, shutdown_network).await;
    }

    pub fn store_path(&self, name: &str) -> PathBuf {
        repository::store_path(&self.store_dir, name)
    }

    pub async fn get_server_config(&self) -> Result<Arc<rustls::ServerConfig>, Error> {
        self.server_config
            .get_or_try_init(|| make_server_config(self.config.dir()))
            .await
            .cloned()
    }

    pub async fn get_client_config(&self) -> Result<Arc<rustls::ClientConfig>, Error> {
        self.client_config
            .get_or_try_init(|| make_client_config(self.config.dir()))
            .await
            .cloned()
    }

    pub async fn delete_repository(&self, name: &str) -> Result<(), Error> {
        if let Some(holder) = self.repositories.remove(name) {
            holder.close().await?;
        }

        repository::delete_store(&self.store_dir, name).await?;

        Ok(())
    }
}

async fn make_server_config(config_dir: &Path) -> Result<Arc<rustls::ServerConfig>, Error> {
    let cert_path = config_dir.join("cert.pem");
    let key_path = config_dir.join("key.pem");

    let certs = transport::tls::load_certificates_from_file(&cert_path)
        .await
        .inspect_err(|error| {
            tracing::error!(
                "failed to load TLS certificate from {}: {}",
                cert_path.display(),
                error,
            )
        })?;

    if certs.is_empty() {
        tracing::error!(
            "failed to load TLS certificate from {}: no certificates found",
            cert_path.display()
        );

        return Err(Error::new(format!(
            "no certificates found in {}",
            cert_path.display()
        )));
    }

    let keys = transport::tls::load_keys_from_file(&key_path)
        .await
        .inspect_err(|error| {
            tracing::error!(
                "failed to load TLS key from {}: {}",
                key_path.display(),
                error
            )
        })?;

    let key = keys.into_iter().next().ok_or_else(|| {
        tracing::error!(
            "failed to load TLS key from {}: no keys found",
            cert_path.display()
        );

        Error::new(format!("no keys found in {}", key_path.display()))
    })?;

    Ok(transport::make_server_config(certs, key)?)
}

async fn make_client_config(config_dir: &Path) -> Result<Arc<rustls::ClientConfig>, Error> {
    // Load custom root certificates (if any)
    let additional_root_certs =
        transport::tls::load_certificates_from_dir(&config_dir.join("root_certs")).await?;
    Ok(transport::make_client_config(&additional_root_certs)?)
}

async fn delete_expired_repositories(state: Weak<State>, poll_interval: Duration) {
    while let Some(state) = state.upgrade() {
        let holders = state.repositories.get_all();

        for holder in holders {
            let last_block_expiration_time =
                match holder.repository.last_block_expiration_time().await {
                    Some(time) => time,
                    None => continue,
                };

            let expiration =
                match ouisync_bridge::repository::get_repository_expiration(&holder.repository)
                    .await
                {
                    Ok(Some(duration)) => duration,
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::error!(?error, "failed to get repository expiration");
                        continue;
                    }
                };

            let elapsed = match last_block_expiration_time.elapsed() {
                Ok(duration) => duration,
                Err(error) => {
                    tracing::error!(
                        ?error,
                        "failed to compute elapsed time since last block expiration"
                    );
                    continue;
                }
            };

            if elapsed < expiration {
                continue;
            }

            match state.delete_repository(holder.name()).await {
                Ok(()) => (),
                Err(error) => {
                    tracing::error!(?error, "failed to delete expired repository");
                }
            }
        }

        drop(state);

        time::sleep(poll_interval).await;
    }
}

use crate::{
    options::Dirs,
    repository::{self, RepositoryMap},
    server::ServerContainer,
    transport::tls,
};
use futures_util::future;
use ouisync_bridge::{
    config::ConfigStore,
    error::Result,
    network::{self, NetworkDefaults},
};
use ouisync_lib::{network::Network, StateMonitor};
use rustls::ServerConfig;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{fs, sync::OnceCell, time};

pub(crate) struct State {
    pub config: ConfigStore,
    pub store_dir: PathBuf,
    pub mount_dir: PathBuf,
    pub network: Network,
    pub repositories: RepositoryMap,
    pub repositories_monitor: StateMonitor,
    pub servers: ServerContainer,
    pub tls_server_config: OnceCell<Option<Arc<ServerConfig>>>,
}

impl State {
    pub async fn init(dirs: &Dirs, monitor: StateMonitor) -> Result<Arc<Self>> {
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
            tls_server_config: OnceCell::new(),
        };
        let state = Arc::new(state);

        state.servers.init(state.clone()).await?;

        Ok(state)
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

    pub fn store_path(&self, name: &str) -> PathBuf {
        repository::store_path(&self.store_dir, name)
    }

    pub async fn get_tls_server_config(&self) -> Result<Option<Arc<ServerConfig>>> {
        self.tls_server_config
            .get_or_try_init(|| make_tls_server_config(self.config.dir()))
            .await
            .map(|config| config.as_ref().cloned())
    }
}

async fn make_tls_server_config(config_dir: &Path) -> Result<Option<Arc<ServerConfig>>> {
    let cert_path = config_dir.join("cert.pem");
    let key_path = config_dir.join("key.pem");

    if !fs::try_exists(&cert_path).await? {
        tracing::warn!("{} not found - continuing without TLS", cert_path.display());
        return Ok(None);
    }

    let certs = tls::load_certificates(&cert_path).await?;
    if certs.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no certificates found in {}", cert_path.display()),
        )
        .into());
    }

    let keys = tls::load_keys(&key_path).await?;
    let key = keys.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("no keys found in {}", key_path.display()),
        )
    })?;

    let config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    tracing::info!("TLS enabled");

    Ok(Some(Arc::new(config)))
}

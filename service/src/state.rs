use ouisync::Network;
use ouisync_bridge::{
    config::ConfigStore,
    network::{self, NetworkDefaults},
    transport::tls,
};
use state_monitor::StateMonitor;
use std::{io, path::Path, sync::Arc};
use thiserror::Error;
use tokio::sync::OnceCell;
use tokio_rustls::rustls;

pub(crate) struct State {
    pub config: ConfigStore,
    pub network: Network,
    remote_server_config: OnceCell<Arc<rustls::ServerConfig>>,
    remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
}

impl State {
    pub async fn init(config_dir: &Path) -> Result<Self, StateError> {
        let config = ConfigStore::new(config_dir);
        let monitor = StateMonitor::make_root();

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

        Ok(Self {
            config,
            network,
            remote_server_config: OnceCell::new(),
            remote_client_config: OnceCell::new(),
        })
    }

    pub async fn remote_server_config(&self) -> Result<Arc<rustls::ServerConfig>, StateError> {
        self.remote_server_config
            .get_or_try_init(|| make_server_config(self.config.dir()))
            .await
            .cloned()
    }

    pub async fn remote_client_config(&self) -> Result<Arc<rustls::ClientConfig>, StateError> {
        self.remote_client_config
            .get_or_try_init(|| make_client_config(self.config.dir()))
            .await
            .cloned()
    }

    pub async fn close(&mut self) -> Result<(), StateError> {
        todo!()
    }
}

#[derive(Error, Debug)]
pub enum StateError {
    #[error("I/O error")]
    Io(#[from] io::Error),
    #[error("TLS certificates not found")]
    TlsCertificatesNotFound,
    #[error("TLS keys not found")]
    TlsKeysNotFound,
}

async fn make_server_config(config_dir: &Path) -> Result<Arc<rustls::ServerConfig>, StateError> {
    let cert_path = config_dir.join("cert.pem");
    let key_path = config_dir.join("key.pem");

    let certs = tls::load_certificates_from_file(&cert_path)
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

        return Err(StateError::TlsCertificatesNotFound);
    }

    let keys = tls::load_keys_from_file(&key_path)
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

        StateError::TlsKeysNotFound
    })?;

    Ok(ouisync_bridge::transport::make_server_config(certs, key)?)
}

async fn make_client_config(config_dir: &Path) -> Result<Arc<rustls::ClientConfig>, StateError> {
    // Load custom root certificates (if any)
    let additional_root_certs =
        tls::load_certificates_from_dir(&config_dir.join("root_certs")).await?;
    Ok(ouisync_bridge::transport::make_client_config(
        &additional_root_certs,
    )?)
}

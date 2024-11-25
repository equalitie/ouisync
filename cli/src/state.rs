/*
use crate::{
    error::Error,
    metrics::MetricsServer,
    options::Dirs,
    repository::{self, RepositoryHolder, RepositoryMap, RepositoryName, OPEN_ON_START},
    server::ServerContainer,
};
use futures_util::future;
use ouisync_bridge::{
    config::{ConfigKey, ConfigStore},
    network::{self, NetworkDefaults},
    transport,
};
use ouisync_lib::{crypto::Password, LocalSecret, Network, SetLocalSecret, ShareToken};
use state_monitor::StateMonitor;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::OnceCell, task, time};

use tokio_rustls::rustls;

pub(crate) struct State {
    pub store_dir: PathBuf,
    pub mount_dir: PathBuf,
    pub repositories: RepositoryMap,
    pub repositories_monitor: StateMonitor,
    pub rpc_servers: ServerContainer,
}

impl State {
    pub async fn init(dirs: &Dirs, monitor: StateMonitor) -> Result<Arc<Self>, Error> {
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
        };
        let state = Arc::new(state);

        state.rpc_servers.init(state.clone()).await?;

        Ok(state)
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

    pub async fn open_repository(
        &self,
        name: RepositoryName,
        password: Option<Password>,
    ) -> Result<(), Error> {
        if self.repositories.contains(&name) {
            Err(Error::RepositoryExists)?;
        }

        let store_path = self.store_path(&name);
        let repository = ouisync_bridge::repository::open(
            store_path,
            password.map(LocalSecret::Password),
            &self.config,
            &self.repositories_monitor,
        )
        .await?;

        let holder = RepositoryHolder::new(repository, name.clone(), &self.network).await;
        let holder = Arc::new(holder);
        if !self.repositories.try_insert(holder.clone()) {
            Err(Error::RepositoryExists)?;
        }

        tracing::info!(%name, "repository opened");

        holder
            .repository
            .metadata()
            .set(OPEN_ON_START, true)
            .await
            .ok();
        holder.mount(&self.mount_dir).await.ok();

        Ok(())
    }

    pub async fn close_repository(&self, name: &str) -> Result<(), Error> {
        let holder = self
            .repositories
            .remove(name)
            .ok_or(Error::RepositoryNotFound)?;

        holder
            .repository
            .metadata()
            .remove(OPEN_ON_START)
            .await
            .ok();

        holder.close().await?;

        Ok(())
    }


    /// Starts task to periodically delete expired repositories.
    pub fn start_delete_expired_repositories(
        self: Arc<Self>,
        poll_interval: Duration,
    ) -> Arc<Self> {
        let state = Arc::downgrade(&self);

        task::spawn(async move {
            while let Some(state) = state.upgrade() {
                state.delete_expired_repositories().await;
                drop(state);
                time::sleep(poll_interval).await;
            }
        });

        self
    }

}


#[expect(clippy::large_enum_variant)]
pub(crate) enum CreateRepositoryMethod {
    Incept { name: String },
    Import { share_token: ShareToken },
}

*/

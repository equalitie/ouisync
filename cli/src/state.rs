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

    async fn delete_expired_repositories(&self) {
        let holders = self.repositories.get_all();

        for holder in holders {
            let Some(last_block_expiration_time) = holder.repository.last_block_expiration_time()
            else {
                continue;
            };

            let expiration = match holder.repository_expiration().await {
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

            match self.delete_repository(holder.name()).await {
                Ok(()) => {
                    tracing::info!(name = %holder.name(), "expired repository deleted");
                }
                Err(error) => {
                    tracing::error!(?error, "failed to delete expired repository");
                }
            }
        }
    }
}


#[expect(clippy::large_enum_variant)]
pub(crate) enum CreateRepositoryMethod {
    Incept { name: String },
    Import { share_token: ShareToken },
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::TryStreamExt;
    use ouisync_lib::{
        Access, AccessSecrets, PeerAddr, Repository, RepositoryParams, WriteSecrets,
    };
    use std::net::Ipv4Addr;
    use tempfile::TempDir;
    use tokio::fs;
    use tokio_stream::wrappers::ReadDirStream;
    use tracing::Instrument;

    #[tokio::test]
    async fn expire_empty_repository() {
        init_log();

        let base_dir = TempDir::new().unwrap();
        let secrets = WriteSecrets::random();

        let state = State::init(
            &Dirs {
                config_dir: base_dir.path().join("config"),
                store_dir: base_dir.path().join("store"),
                mount_dir: base_dir.path().join("mount"),
            },
            StateMonitor::make_root(),
        )
        .await
        .unwrap();

        let name = "foo";
        let holder = state
            .create_repository(
                CreateRepositoryMethod::Import {
                    share_token: ShareToken::from(AccessSecrets::Blind { id: secrets.id })
                        .with_name(name),
                },
                None,
                None,
            )
            .await
            .unwrap();

        // Repository expiration requires block expiration to be enabled as well.
        holder
            .repository
            .set_block_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();

        holder
            .set_repository_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();

        drop(holder);

        time::sleep(Duration::from_secs(1)).await;

        state.delete_expired_repositories().await;

        assert!(!state.repositories.contains(name));
        assert_eq!(
            read_dir(base_dir.path().join("store")).await,
            Vec::<PathBuf>::new()
        );
    }

    #[tokio::test]
    async fn expire_synced_repository() {
        init_log();

        let base_dir = TempDir::new().unwrap();

        let secrets = WriteSecrets::random();
        let monitor = StateMonitor::make_root();

        let (remote_network, _remote_repo, _remote_reg) = async {
            let monitor = monitor.make_child("remote");

            let repo = Repository::create(
                &RepositoryParams::new(base_dir.path().join("remote/repo.ouisyncdb"))
                    .with_parent_monitor(monitor.clone()),
                Access::WriteUnlocked {
                    secrets: secrets.clone(),
                },
            )
            .await
            .unwrap();

            let mut file = repo.create_file("test.txt").await.unwrap();
            file.write_all(b"hello world").await.unwrap();
            file.flush().await.unwrap();
            drop(file);

            let network = Network::new(monitor, None, None);
            network
                .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
                .await;

            let reg = network.register(repo.handle()).await;

            (network, repo, reg)
        }
        .instrument(tracing::info_span!("remote"))
        .await;

        let remote_addr = remote_network
            .listener_local_addrs()
            .into_iter()
            .next()
            .unwrap();

        let local_state = State::init(
            &Dirs {
                config_dir: base_dir.path().join("local/config"),
                store_dir: base_dir.path().join("local/store"),
                mount_dir: base_dir.path().join("local/mount"),
            },
            monitor.make_child("local"),
        )
        .instrument(tracing::info_span!("local"))
        .await
        .unwrap();

        local_state
            .network
            .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;
        local_state.network.add_user_provided_peer(&remote_addr);

        let name = "foo";
        let holder = local_state
            .create_repository(
                CreateRepositoryMethod::Import {
                    share_token: ShareToken::from(AccessSecrets::Blind { id: secrets.id })
                        .with_name(name),
                },
                None,
                None,
            )
            .await
            .unwrap();

        // Wait until synced
        let mut rx = holder.repository.subscribe();

        time::timeout(Duration::from_secs(30), async {
            loop {
                let progress = holder.repository.sync_progress().await.unwrap();

                if progress.total > 0 && progress.value == progress.total {
                    break;
                }

                rx.recv().await.unwrap();
            }
        })
        .await
        .unwrap();

        // Enable expiration
        holder
            .repository
            .set_block_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();
        holder
            .set_repository_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();

        drop(holder);

        time::sleep(Duration::from_secs(1)).await;

        local_state.delete_expired_repositories().await;

        assert!(!local_state.repositories.contains(name));
        assert_eq!(
            read_dir(base_dir.path().join("local/store")).await,
            Vec::<PathBuf>::new()
        );
    }

    fn init_log() {
        tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_target(false)
            .with_test_writer()
            .try_init()
            .ok();
    }

    async fn read_dir(path: impl AsRef<Path>) -> Vec<PathBuf> {
        ReadDirStream::new(fs::read_dir(path).await.unwrap())
            .map_ok(|entry| entry.path())
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
    }
}
*/

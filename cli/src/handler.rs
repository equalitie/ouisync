use crate::{
    options::{Dirs, Request, Response},
    repository::{self, RepositoryHolder, RepositoryMap, OPEN_ON_START},
};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use futures_util::future;
use ouisync_bridge::{
    config::ConfigStore,
    error::{Error, Result},
    network::{self, NetworkDefaults},
    transport::NotificationSender,
};
use ouisync_lib::{network::Network, PeerAddr, ShareToken, StateMonitor};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::time;

pub(crate) struct State {
    config: ConfigStore,
    store_dir: Utf8PathBuf,
    mount_dir: Utf8PathBuf,
    network: Network,
    repositories: RepositoryMap,
    repositories_monitor: StateMonitor,
}

impl State {
    pub async fn new(dirs: &Dirs, monitor: StateMonitor) -> Self {
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

        Self {
            config,
            store_dir: dirs.store_dir.clone(),
            mount_dir: dirs.mount_dir.clone(),
            network,
            repositories,
            repositories_monitor,
        }
    }

    pub async fn close(&self) {
        // Close repos
        future::join_all(
            self.repositories
                .remove_all()
                .into_iter()
                .map(|holder| async move {
                    if let Err(error) = holder.repository.close().await {
                        tracing::error!(
                            name = holder.name(),
                            ?error,
                            "failed to gracefully close repository"
                        );
                    }
                }),
        )
        .await;

        time::timeout(Duration::from_secs(1), self.network.shutdown())
            .await
            .ok();
    }

    fn store_path(&self, name: &str) -> Utf8PathBuf {
        repository::store_path(&self.store_dir, name)
    }
}

#[derive(Clone)]
pub(crate) struct Handler {
    state: Arc<State>,
}

impl Handler {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }

    pub async fn close(&self) {
        self.state.close().await
    }
}

#[async_trait]
impl ouisync_bridge::transport::Handler for Handler {
    type Request = Request;
    type Response = Response;

    async fn handle(
        &self,
        request: Self::Request,
        _notification_tx: &NotificationSender,
    ) -> Result<Self::Response> {
        tracing::debug!(?request);

        match request {
            Request::Serve => Err(Error::ForbiddenRequest),
            Request::Create {
                name,
                share_token,
                password,
                read_password,
                write_password,
            } => {
                let share_token = share_token
                    .as_deref()
                    .map(str::parse::<ShareToken>)
                    .transpose()
                    .map_err(|_| Error::InvalidArgument)?;

                let name = match (name, &share_token) {
                    (Some(name), _) => name,
                    (None, Some(token)) => token.suggested_name().into_owned(),
                    (None, None) => unreachable!(),
                };

                if self.state.repositories.contains(&name) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

                let store_path = self.state.store_path(&name);
                let read_password = read_password.or_else(|| password.as_ref().cloned());
                let write_password = write_password.or(password);

                let repository = ouisync_bridge::repository::create(
                    store_path.clone(),
                    read_password,
                    write_password,
                    share_token,
                    &self.state.config,
                    &self.state.repositories_monitor,
                )
                .await?;

                repository.metadata().set(OPEN_ON_START, true).await.ok();

                tracing::info!(name, "repository created");

                let holder = RepositoryHolder::new(repository, name, &self.state.network).await;
                let holder = Arc::new(holder);
                self.state.repositories.insert(holder);

                Ok(().into())
            }
            Request::Delete { name } => {
                self.state.repositories.remove(&name);

                repository::delete_store(&self.state.store_dir, &name)
                    .await
                    .map_err(Error::Io)?;

                Ok(().into())
            }
            Request::Open { name, password } => {
                if self.state.repositories.contains(&name) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

                let store_path = self.state.store_path(&name);

                let repository = ouisync_bridge::repository::open(
                    store_path,
                    password,
                    &self.state.config,
                    &self.state.repositories_monitor,
                )
                .await?;

                repository.metadata().set(OPEN_ON_START, true).await.ok();

                tracing::info!(name, "repository opened");

                let holder = RepositoryHolder::new(repository, name, &self.state.network).await;
                let holder = Arc::new(holder);
                holder.mount(&self.state.mount_dir).await.ok();

                self.state.repositories.insert(holder);

                Ok(().into())
            }
            Request::Close { name } => {
                let holder = self
                    .state
                    .repositories
                    .remove(&name)
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                holder
                    .repository
                    .metadata()
                    .remove(OPEN_ON_START)
                    .await
                    .ok();

                Ok(().into())
            }
            Request::Share {
                name,
                mode,
                password,
            } => {
                let holder = self
                    .state
                    .repositories
                    .get(&name)
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                ouisync_bridge::repository::create_share_token(
                    &holder.repository,
                    password,
                    mode,
                    Some(name),
                )
                .await
                .map(Into::into)
            }
            Request::Mount { name, path, all: _ } => {
                if let Some(name) = name {
                    let holder = self
                        .state
                        .repositories
                        .get(&name)
                        .ok_or(ouisync_lib::Error::EntryNotFound)?;

                    let mount_point = path.as_ref().map(|path| path.as_str()).unwrap_or_default();
                    holder.set_mount_point(Some(mount_point)).await;
                    holder.mount(&self.state.mount_dir).await?;
                } else {
                    for holder in self.state.repositories.get_all() {
                        if holder.is_mounted() {
                            continue;
                        }

                        holder.set_mount_point(Some("")).await;
                        holder.mount(&self.state.mount_dir).await?;
                    }
                }

                Ok(().into())
            }
            Request::Unmount { name, all: _ } => {
                if let Some(name) = name {
                    if let Some(holder) = self.state.repositories.get(&name) {
                        holder.set_mount_point(None).await;
                        holder.unmount().await;
                    }
                } else {
                    for holder in self.state.repositories.get_all() {
                        holder.set_mount_point(None).await;
                        holder.unmount().await;
                    }
                }

                Ok(().into())
            }
            Request::Bind { addrs } => {
                network::bind(&self.state.network, &self.state.config, &addrs).await;
                Ok(().into())
            }
            Request::ListPorts => {
                let ports: Vec<_> = self
                    .state
                    .network
                    .listener_local_addrs()
                    .into_iter()
                    .map(|addr| match addr {
                        PeerAddr::Quic(SocketAddr::V4(addr)) => {
                            format!("QUIC, IPv4: {}", addr.port())
                        }
                        PeerAddr::Quic(SocketAddr::V6(addr)) => {
                            format!("QUIC, IPv6: {}", addr.port())
                        }
                        PeerAddr::Tcp(SocketAddr::V4(addr)) => {
                            format!("TCP, IPv4: {}", addr.port())
                        }
                        PeerAddr::Tcp(SocketAddr::V6(addr)) => {
                            format!("TCP, IPv6: {}", addr.port())
                        }
                    })
                    .collect();

                Ok(ports.into())
            }
            Request::LocalDiscovery { enabled } => {
                if let Some(enabled) = enabled {
                    network::set_local_discovery_enabled(
                        &self.state.network,
                        &self.state.config,
                        enabled,
                    )
                    .await;
                    Ok(().into())
                } else {
                    Ok(self.state.network.is_local_discovery_enabled().into())
                }
            }
            Request::PortForwarding { enabled } => {
                if let Some(enabled) = enabled {
                    network::set_port_forwarding_enabled(
                        &self.state.network,
                        &self.state.config,
                        enabled,
                    )
                    .await;
                    Ok(().into())
                } else {
                    Ok(self.state.network.is_port_forwarding_enabled().into())
                }
            }
            Request::AddPeers { addrs } => {
                network::add_user_provided_peers(&self.state.network, &self.state.config, &addrs)
                    .await;
                Ok(().into())
            }
            Request::RemovePeers { addrs } => {
                network::remove_user_provided_peers(
                    &self.state.network,
                    &self.state.config,
                    &addrs,
                )
                .await;
                Ok(().into())
            }
            Request::ListPeers => Ok(self.state.network.collect_peer_info().into()),
            Request::Dht {
                repository_name,
                enabled,
            } => {
                let holder = self
                    .state
                    .repositories
                    .get(&repository_name)
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                if let Some(enabled) = enabled {
                    holder.registration.set_dht_enabled(enabled).await;
                    Ok(().into())
                } else {
                    Ok(holder.registration.is_dht_enabled().into())
                }
            }
            Request::Pex {
                repository_name,
                enabled,
            } => {
                let holder = self
                    .state
                    .repositories
                    .get(&repository_name)
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                if let Some(enabled) = enabled {
                    holder.registration.set_pex_enabled(enabled).await;
                    Ok(().into())
                } else {
                    Ok(holder.registration.is_pex_enabled().into())
                }
            }
        }
    }
}

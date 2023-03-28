use crate::{
    options::{Dirs, Request, Response},
    APP_NAME,
};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use dashmap::{mapref::entry::Entry, DashMap};
use futures_util::future;
use ouisync_bridge::{
    config::ConfigStore,
    error::{Error, Result},
    network::{self, NetworkDefaults},
    repository,
    transport::NotificationSender,
};
use ouisync_lib::{network::Network, PeerAddr, ShareToken};
use ouisync_vfs::MountGuard;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{fs, runtime, time};

pub(crate) struct State {
    config: ConfigStore,
    store_dir: Utf8PathBuf,
    mount_dir: Utf8PathBuf,
    network: Network,
    repositories: DashMap<String, RepositoryHolder>,
}

impl State {
    pub async fn new(dirs: &Dirs) -> Self {
        let config = ConfigStore::new(&dirs.config_dir);

        let network = {
            let _enter = tracing::info_span!("Network").entered();
            Network::new()
        };

        network::init(
            &network,
            &config,
            NetworkDefaults {
                port_forwarding_enabled: false,
                local_discovery_enabled: false,
            },
        )
        .await;

        Self {
            config,
            store_dir: dirs.store_dir.clone(),
            mount_dir: dirs.mount_dir.clone(),
            network,
            repositories: DashMap::new(),
        }
    }

    pub async fn close(&self) {
        let mut repositories = Vec::with_capacity(self.repositories.len());

        self.repositories.retain(|path, holder| {
            repositories.push((path.clone(), holder.base.repository.clone()));
            false
        });

        future::join_all(
            repositories
                .into_iter()
                .map(|(path, repository)| async move {
                    if let Err(error) = repository.close().await {
                        tracing::error!(?error, ?path, "failed to close repository");
                    }
                }),
        )
        .await;

        time::timeout(Duration::from_secs(1), self.network.shutdown())
            .await
            .ok();
    }

    fn store_path(&self, name: &str) -> Utf8PathBuf {
        self.store_dir
            .join(name)
            .with_extension(format!("{APP_NAME}db"))
    }

    fn mount_path(&self, name: &str) -> Utf8PathBuf {
        self.mount_dir.join(name)
    }
}

struct RepositoryHolder {
    base: ouisync_bridge::repository::RepositoryHolder,
    mount_guard: Option<MountGuard>,
}

impl RepositoryHolder {
    fn new(base: ouisync_bridge::repository::RepositoryHolder) -> Self {
        Self {
            base,
            mount_guard: None,
        }
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

                if self.state.repositories.contains_key(&name) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

                let store_path = self.state.store_path(&name);
                let read_password = read_password.or_else(|| password.as_ref().cloned());
                let write_password = write_password.or(password);

                let holder = repository::create(
                    store_path.clone(),
                    read_password,
                    write_password,
                    share_token,
                    &self.state.config,
                    &self.state.network,
                )
                .await?;
                let holder = RepositoryHolder::new(holder);

                self.state.repositories.insert(name, holder);

                Ok(().into())
            }
            Request::Delete { name } => {
                if let Some((_, holder)) = self.state.repositories.remove(&name) {
                    if let Err(error) = holder.base.repository.close().await {
                        tracing::error!(?error, "failed to close repository");
                    }
                }

                let store_path = self.state.store_path(&name);

                // Try to delete all three files even if any of them fail, then return the first
                // error (if any)
                future::join_all(
                    ["", "-wal", "-shm"]
                        .into_iter()
                        .map(|suffix| fs::remove_file(format!("{store_path}-{suffix}"))),
                )
                .await
                .into_iter()
                .find_map(Result::err)
                .map(Err)
                .unwrap_or(Ok(().into()))
                .map_err(Error::Io)
            }
            Request::Open { name, password } => {
                if self.state.repositories.contains_key(&name) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

                let store_path = self.state.store_path(&name);

                let holder = repository::open(
                    store_path,
                    password,
                    &self.state.config,
                    &self.state.network,
                )
                .await?;
                let holder = RepositoryHolder::new(holder);

                match self.state.repositories.entry(name) {
                    Entry::Vacant(entry) => {
                        entry.insert(holder);
                        Ok(().into())
                    }
                    Entry::Occupied(_) => Err(ouisync_lib::Error::EntryExists.into()),
                }
            }
            Request::Close { name } => {
                self.state
                    .repositories
                    .remove(&name)
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                Ok(().into())
            }
            Request::Share {
                name,
                mode,
                password,
            } => {
                let repository = self
                    .state
                    .repositories
                    .get(&name)
                    .map(|r| r.value().base.repository.clone())
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                repository::create_share_token(&repository, password, mode, Some(name))
                    .await
                    .map(Into::into)
            }
            Request::Mount { name, path, all: _ } => {
                if let Some(name) = name {
                    let mut holder = self
                        .state
                        .repositories
                        .get_mut(&name)
                        .ok_or(ouisync_lib::Error::EntryNotFound)?;

                    let mount_path = path.unwrap_or_else(|| self.state.mount_path(&name));
                    holder.mount_guard = Some(ouisync_vfs::mount(
                        runtime::Handle::current(),
                        holder.base.repository.clone(),
                        mount_path,
                    )?);
                } else {
                    for mut entry in self.state.repositories.iter_mut() {
                        if entry.mount_guard.is_some() {
                            continue;
                        }

                        let mount_path = self.state.mount_path(entry.key());
                        fs::create_dir_all(&mount_path).await?;

                        entry.mount_guard = Some(ouisync_vfs::mount(
                            runtime::Handle::current(),
                            entry.base.repository.clone(),
                            mount_path,
                        )?);
                    }
                }

                Ok(().into())
            }
            Request::Unmount { name, all: _ } => {
                if let Some(name) = name {
                    if let Some(mut holder) = self.state.repositories.get_mut(&name) {
                        holder.mount_guard.take();
                    }
                } else {
                    for mut entry in self.state.repositories.iter_mut() {
                        entry.mount_guard.take();
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
                    repository::set_dht_enabled(&holder.base.registration, enabled);
                    Ok(().into())
                } else {
                    Ok(holder.base.registration.is_dht_enabled().into())
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
                    repository::set_pex_enabled(&holder.base.registration, enabled);
                    Ok(().into())
                } else {
                    Ok(holder.base.registration.is_pex_enabled().into())
                }
            }
        }
    }
}

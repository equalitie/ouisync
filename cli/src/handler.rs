use crate::{
    async_walkdir,
    options::{Dirs, Request, Response},
    DB_EXTENSION,
};
use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use dashmap::{mapref::entry::Entry, DashMap};
use futures_util::{future, StreamExt};
use ouisync_bridge::{
    config::ConfigStore,
    error::{Error, Result},
    network::{self, NetworkDefaults},
    repository,
    transport::NotificationSender,
};
use ouisync_lib::{
    network::{Network, Registration},
    PeerAddr, Repository, ShareToken, StateMonitor,
};
use ouisync_vfs::MountGuard;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{fs, runtime, time};

const OPEN_ON_START: &str = "open_on_start";

pub(crate) struct State {
    config: ConfigStore,
    store_dir: Utf8PathBuf,
    mount_dir: Utf8PathBuf,
    network: Network,
    repositories: DashMap<String, RepositoryHolder>,
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
            open_repositories(&dirs.store_dir, &network, &config, &repositories_monitor).await;

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
        let mut repositories = Vec::with_capacity(self.repositories.len());

        self.repositories.retain(|path, holder| {
            repositories.push((path.clone(), holder.repository.clone()));
            false
        });

        future::join_all(
            repositories
                .into_iter()
                .map(|(name, repository)| async move {
                    if let Err(error) = repository.close().await {
                        tracing::error!(?name, ?error, "failed to gracefully close repository");
                    }
                }),
        )
        .await;

        time::timeout(Duration::from_secs(1), self.network.shutdown())
            .await
            .ok();
    }

    fn store_path(&self, name: &str) -> Utf8PathBuf {
        self.store_dir.join(name).with_extension(DB_EXTENSION)
    }

    fn mount_path(&self, name: &str) -> Utf8PathBuf {
        self.mount_dir.join(name)
    }
}

struct RepositoryHolder {
    repository: Arc<Repository>,
    registration: Registration,
    mount_guard: Option<MountGuard>,
}

impl RepositoryHolder {
    async fn new(repository: Repository, network: &Network) -> Self {
        let repository = Arc::new(repository);
        let registration = network.register(repository.store().clone()).await;

        Self {
            repository,
            registration,
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

                let repository = repository::create(
                    store_path.clone(),
                    read_password,
                    write_password,
                    share_token,
                    &self.state.config,
                    &self.state.repositories_monitor,
                )
                .await?;

                repository.metadata().set(OPEN_ON_START, true).await.ok();

                tracing::info!(?name, "repository created");

                let holder = RepositoryHolder::new(repository, &self.state.network).await;
                self.state.repositories.insert(name, holder);

                Ok(().into())
            }
            Request::Delete { name } => {
                if let Some((_, holder)) = self.state.repositories.remove(&name) {
                    if let Err(error) = holder.repository.close().await {
                        tracing::error!(?name, ?error, "failed to gracefully close repository");
                    }

                    tracing::info!(?name, "repository closed");
                }

                let store_path = self.state.store_path(&name);

                ouisync_lib::delete_repository(store_path)
                    .await
                    .map_err(Error::Io)?;

                Ok(().into())
            }
            Request::Open { name, password } => {
                if self.state.repositories.contains_key(&name) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

                let store_path = self.state.store_path(&name);

                let repository = repository::open(
                    store_path,
                    password,
                    &self.state.config,
                    &self.state.repositories_monitor,
                )
                .await?;

                repository.metadata().set(OPEN_ON_START, true).await.ok();

                tracing::info!(?name, "repository opened");

                match self.state.repositories.entry(name) {
                    Entry::Vacant(entry) => {
                        let holder = RepositoryHolder::new(repository, &self.state.network).await;
                        entry.insert(holder);
                        Ok(().into())
                    }
                    Entry::Occupied(_) => Err(ouisync_lib::Error::EntryExists.into()),
                }
            }
            Request::Close { name } => {
                let (_, holder) = self
                    .state
                    .repositories
                    .remove(&name)
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                holder
                    .repository
                    .metadata()
                    .set(OPEN_ON_START, false) // TODO: use remove()
                    .await
                    .ok();

                if let Err(error) = holder.repository.close().await {
                    tracing::error!(?name, ?error, "failed to gracefully close repository");
                }

                tracing::info!(?name, "repository closed");

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
                    .map(|r| r.value().repository.clone())
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
                        holder.repository.clone(),
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
                            entry.repository.clone(),
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

// Find repositories that are marked to be opened on startup and open them.
async fn open_repositories(
    store_path: &Utf8Path,
    network: &Network,
    config: &ConfigStore,
    monitor: &StateMonitor,
) -> DashMap<String, RepositoryHolder> {
    let mut walkdir = async_walkdir::new(store_path);
    let repositories = DashMap::new();

    while let Some(entry) = walkdir.next().await {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::error!(%error, "failed to read directory entry");
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path: &Utf8Path = match entry.path().try_into() {
            Ok(path) => path,
            Err(_) => {
                tracing::error!(path = ?entry.path(), "invalid repository path - not utf8");
                continue;
            }
        };

        if path.extension() != Some(DB_EXTENSION) {
            continue;
        }

        let repository = match repository::open(path.to_path_buf(), None, config, monitor).await {
            Ok(repository) => repository,
            Err(error) => {
                tracing::error!(?error, ?path, "failed to open repository");
                continue;
            }
        };

        let metadata = repository.metadata();

        if !metadata.get(OPEN_ON_START).await.unwrap_or(false) {
            continue;
        }

        let name = path.strip_prefix(store_path).unwrap_or(path).as_str();

        tracing::info!(?name, "repository opened");

        let holder = RepositoryHolder::new(repository, network).await;
        repositories.insert(name.to_owned(), holder);
    }

    repositories
}

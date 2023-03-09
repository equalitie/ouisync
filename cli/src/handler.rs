use crate::{
    options::{Dirs, Request, Response},
    APP_NAME,
};
use async_trait::async_trait;
use camino::Utf8PathBuf;
use dashmap::DashMap;
use futures_util::future;
use ouisync_bridge::{network, repository, transport::NotificationSender, Error, Result};
use ouisync_lib::{network::Network, ConfigStore, PeerAddr, ShareToken};
use ouisync_vfs::MountGuard;
use std::{net::SocketAddr, sync::Arc};
use tokio::{fs, runtime};

pub(crate) struct State {
    config: ConfigStore,
    store_dir: Utf8PathBuf,
    mount_dir: Utf8PathBuf,
    network: Network,
    repositories: DashMap<Utf8PathBuf, RepositoryHolder>,
}

impl State {
    pub fn new(dirs: &Dirs) -> Self {
        let config = ConfigStore::new(&dirs.config_dir);

        let network = {
            let _enter = tracing::info_span!("Network").entered();
            Network::new(config.clone())
        };

        Self {
            config,
            store_dir: dirs.store_dir.clone(),
            mount_dir: dirs.mount_dir.clone(),
            network,
            repositories: DashMap::new(),
        }
    }

    pub async fn close(&self) {
        todo!()
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
    base: ouisync_bridge::RepositoryHolder,
    mount_guard: Option<MountGuard>,
}

impl RepositoryHolder {
    fn new(base: ouisync_bridge::RepositoryHolder) -> Self {
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

                let read_password = read_password.or_else(|| password.as_ref().cloned());
                let write_password = write_password.or(password);

                let name = match (name, &share_token) {
                    (Some(name), _) => name,
                    (None, Some(token)) => token.suggested_name().into_owned(),
                    (None, None) => unreachable!(),
                };

                let store_path = self.state.store_path(&name);

                if self.state.repositories.contains_key(&store_path) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

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

                self.state.repositories.insert(store_path, holder);

                Ok(().into())
            }
            Request::Delete { name } => {
                let store_path = self.state.store_path(&name);

                if let Some((_, holder)) = self.state.repositories.remove(&store_path) {
                    if let Err(error) = holder.base.repository.close().await {
                        tracing::error!(?error, "failed to close repository");
                    }
                }

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
            Request::Share {
                name,
                mode,
                password,
            } => {
                let store_path = self.state.store_path(&name);
                let repository = self
                    .state
                    .repositories
                    .get(&store_path)
                    .map(|r| r.value().base.repository.clone())
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                repository::create_share_token(&repository, password, mode, Some(name))
                    .await
                    .map(Into::into)
            }
            Request::Mount { name, path } => {
                let store_path = self.state.store_path(&name);
                let mount_path = path.unwrap_or_else(|| self.state.mount_path(&name));

                let mut holder = self
                    .state
                    .repositories
                    .get_mut(&store_path)
                    .ok_or(ouisync_lib::Error::EntryNotFound)?;

                holder.mount_guard = Some(ouisync_vfs::mount(
                    runtime::Handle::current(),
                    holder.base.repository.clone(),
                    mount_path,
                )?);

                Ok(().into())
            }
            Request::Unmount { name } => {
                let store_path = self.state.store_path(&name);

                if let Some(mut holder) = self.state.repositories.get_mut(&store_path) {
                    holder.mount_guard.take();
                }

                Ok(().into())
            }
            Request::Bind { addrs } => {
                let mut quic_v4 = None;
                let mut quic_v6 = None;
                let mut tcp_v4 = None;
                let mut tcp_v6 = None;

                for addr in addrs {
                    match addr {
                        PeerAddr::Quic(SocketAddr::V4(addr)) => quic_v4 = Some(addr),
                        PeerAddr::Quic(SocketAddr::V6(addr)) => quic_v6 = Some(addr),
                        PeerAddr::Tcp(SocketAddr::V4(addr)) => tcp_v4 = Some(addr),
                        PeerAddr::Tcp(SocketAddr::V6(addr)) => tcp_v6 = Some(addr),
                    }
                }

                network::bind(&self.state.network, quic_v4, quic_v6, tcp_v4, tcp_v6).await;

                Ok(().into())
            }
            Request::LocalDiscovery { enabled } => {
                if let Some(enabled) = enabled {
                    network::set_local_discovery_enabled(&self.state.network, enabled);
                    Ok(().into())
                } else {
                    Ok(self.state.network.is_local_discovery_enabled().into())
                }
            }
            Request::PortForwarding { enabled } => {
                if let Some(enabled) = enabled {
                    network::set_port_forwarding_enabled(&self.state.network, enabled);
                    Ok(().into())
                } else {
                    Ok(self.state.network.is_port_forwarding_enabled().into())
                }
            }
            Request::AddPeers { addrs } => {
                for addr in addrs {
                    self.state.network.add_user_provided_peer(&addr);
                }

                Ok(().into())
            }
            Request::RemovePeers { addrs } => {
                for addr in addrs {
                    self.state.network.remove_user_provided_peer(&addr);
                }

                Ok(().into())
            }
            Request::ListPeers => Ok(self.state.network.collect_peer_info().into()),
        }
    }
}

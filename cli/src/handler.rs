use crate::{
    protocol::{Request, Response},
    repository::{self, RepositoryHolder, RepositoryName, OPEN_ON_START},
    state::State,
};
use async_trait::async_trait;
use ouisync_bridge::{
    error::{Error, Result},
    network,
    transport::NotificationSender,
};
use ouisync_lib::{AccessMode, PeerAddr, ShareToken};
use std::{
    net::SocketAddr,
    sync::{Arc, Weak},
};

#[derive(Clone)]
pub(crate) struct LocalHandler {
    state: Arc<State>,
}

impl LocalHandler {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }

    pub async fn close(&self) {
        self.state.close().await
    }
}

#[async_trait]
impl ouisync_bridge::transport::Handler for LocalHandler {
    type Request = Request;
    type Response = Response;

    async fn handle(
        &self,
        request: Self::Request,
        _notification_tx: &NotificationSender,
    ) -> Result<Self::Response> {
        tracing::debug!(?request);

        match request {
            Request::Start => Err(Error::ForbiddenRequest),
            Request::BindRpc { addrs } => Ok(self
                .state
                .servers
                .set(addrs, self.state.clone())
                .await
                .into()),
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

                let name = RepositoryName::try_from(name)?;

                let store_path = self.state.store_path(name.as_ref());
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

                tracing::info!(%name, "repository created");

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

                let name = RepositoryName::try_from(name)?;

                let store_path = self.state.store_path(&name);

                let repository = ouisync_bridge::repository::open(
                    store_path,
                    password,
                    &self.state.config,
                    &self.state.repositories_monitor,
                )
                .await?;

                repository.metadata().set(OPEN_ON_START, true).await.ok();

                tracing::info!(%name, "repository opened");

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

#[derive(Clone)]
pub(crate) struct RemoteHandler {
    state: Weak<State>,
}

impl RemoteHandler {
    pub fn new(state: Arc<State>) -> Self {
        Self {
            state: Arc::downgrade(&state),
        }
    }
}

#[async_trait]
impl ouisync_bridge::transport::Handler for RemoteHandler {
    type Request = Request;
    type Response = Response;

    async fn handle(
        &self,
        request: Self::Request,
        _notification_tx: &NotificationSender,
    ) -> Result<Self::Response> {
        tracing::debug!(?request);

        let Some(state) = self.state.upgrade() else {
            tracing::error!("can't handle request - shutting down");
            // TODO: return more appropriate error (ShuttingDown or similar)
            return Err(Error::ForbiddenRequest);
        };

        match request {
            Request::Create {
                share_token: Some(share_token),
                name: None,
                password: None,
                read_password: None,
                write_password: None,
            } => {
                let share_token: ShareToken =
                    share_token.parse().map_err(|_| Error::InvalidArgument)?;
                // We support remote creation of blind replicas only.
                let share_token: ShareToken = share_token
                    .into_secrets()
                    .with_mode(AccessMode::Blind)
                    .into();

                let name = share_token
                    .id()
                    .salted_hash(b"ouisync server repository name")
                    .to_string();
                // unwrap ok because the name is just a string of hexadecimal digits which is
                // always a valid name.
                let name = RepositoryName::try_from(name).unwrap();

                let store_path = state.store_path(name.as_ref());

                let repository = ouisync_bridge::repository::create(
                    store_path.clone(),
                    None,
                    None,
                    Some(share_token),
                    &state.config,
                    &state.repositories_monitor,
                )
                .await?;

                repository.metadata().set(OPEN_ON_START, true).await.ok();

                tracing::info!(%name, "repository created");

                let holder = RepositoryHolder::new(repository, name, &state.network).await;
                let holder = Arc::new(holder);
                state.repositories.insert(holder);

                Ok(().into())
            }
            _ => Err(Error::ForbiddenRequest),
        }
    }
}

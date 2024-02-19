use crate::{
    protocol::{Error, QuotaInfo, Request, Response},
    repository::{self, RepositoryHolder, RepositoryName, OPEN_ON_START},
    state::State,
};
use async_trait::async_trait;
use ouisync_bridge::{network, transport::NotificationSender};
use ouisync_lib::{crypto::Password, LocalSecret, PeerAddr, SetLocalSecret, ShareToken};
use std::{net::SocketAddr, sync::Arc, time::Duration};

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
    type Error = Error;

    async fn handle(
        &self,
        request: Self::Request,
        _notification_tx: &NotificationSender,
    ) -> Result<Self::Response, Self::Error> {
        match request {
            Request::Start { .. } => unimplemented!(),
            Request::BindRpc { addrs } => Ok(self
                .state
                .rpc_servers
                .set(self.state.clone(), &addrs)
                .await?
                .into()),
            Request::BindMetrics { addr } => Ok(self
                .state
                .metrics_server
                .bind(&self.state, addr)
                .await?
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
                    .map_err(|error| Error::new(format!("invalid share token: {error}")))?;

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
                    store_path,
                    read_password
                        .map(Password::from)
                        .map(SetLocalSecret::Password),
                    write_password
                        .map(Password::from)
                        .map(SetLocalSecret::Password),
                    share_token,
                    &self.state.config,
                    &self.state.repositories_monitor,
                )
                .await?;

                let holder =
                    RepositoryHolder::new(repository, name.clone(), &self.state.network).await;
                let holder = Arc::new(holder);

                if !self.state.repositories.try_insert(holder.clone()) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

                holder
                    .repository
                    .metadata()
                    .set(OPEN_ON_START, true)
                    .await
                    .ok();

                tracing::info!(%name, "repository created");

                Ok(().into())
            }
            Request::Delete { name } => {
                self.state.repositories.remove(&name);

                repository::delete_store(&self.state.store_dir, &name).await?;

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
                    password.map(Password::from).map(LocalSecret::Password),
                    &self.state.config,
                    &self.state.repositories_monitor,
                )
                .await?;

                let holder =
                    RepositoryHolder::new(repository, name.clone(), &self.state.network).await;
                let holder = Arc::new(holder);
                if !self.state.repositories.try_insert(holder.clone()) {
                    Err(ouisync_lib::Error::EntryExists)?;
                }

                tracing::info!(%name, "repository opened");

                holder
                    .repository
                    .metadata()
                    .set(OPEN_ON_START, true)
                    .await
                    .ok();
                holder.mount(&self.state.mount_dir).await.ok();

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
                let holder = self.state.repositories.find(&name)?;
                let token = ouisync_bridge::repository::create_share_token(
                    &holder.repository,
                    password.map(Password::from).map(LocalSecret::Password),
                    mode,
                    Some(name),
                )
                .await?;

                Ok(token.into())
            }
            Request::Mount { name, path, all: _ } => {
                if let Some(name) = name {
                    let holder = self.state.repositories.find(&name)?;

                    let mount_point = if let Some(path) = &path {
                        path.to_str()
                            .ok_or_else(|| Error::new("invalid mount point"))?
                    } else {
                        ""
                    };

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
                    if let Ok(holder) = self.state.repositories.find(&name) {
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
            Request::Mirror { name, host } => {
                let holder = self.state.repositories.find(&name)?;
                let config = self.state.get_client_config().await?;

                holder.mirror(&host, config).await?;

                Ok(().into())
            }
            Request::ListRepositories => {
                let names: Vec<_> = self
                    .state
                    .repositories
                    .get_all()
                    .into_iter()
                    .map(|holder| holder.name().to_string())
                    .collect();
                Ok(names.into())
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
            Request::ListPeers => Ok(self.state.network.peer_info_collector().collect().into()),
            Request::Dht { name, enabled } => {
                let holder = self.state.repositories.find(&name)?;

                if let Some(enabled) = enabled {
                    holder.registration.set_dht_enabled(enabled).await;
                    Ok(().into())
                } else {
                    Ok(holder.registration.is_dht_enabled().into())
                }
            }
            Request::Pex { name, enabled } => {
                let holder = self.state.repositories.find(&name)?;

                if let Some(enabled) = enabled {
                    holder.registration.set_pex_enabled(enabled).await;
                    Ok(().into())
                } else {
                    Ok(holder.registration.is_pex_enabled().into())
                }
            }
            Request::Quota {
                name,
                default: _,
                remove,
                value,
            } => {
                let value = if remove { Some(None) } else { value.map(Some) };

                if let Some(name) = name {
                    let holder = self.state.repositories.find(&name)?;

                    if let Some(value) = value {
                        holder.repository.set_quota(value).await?;
                        Ok(().into())
                    } else {
                        let quota = holder.repository.quota().await?;
                        let size = holder.repository.size().await?;

                        Ok(QuotaInfo { quota, size }.into())
                    }
                } else if let Some(value) = value {
                    ouisync_bridge::repository::set_default_quota(&self.state.config, value)
                        .await?;
                    Ok(().into())
                } else if let Some(value) =
                    ouisync_bridge::repository::get_default_quota(&self.state.config).await?
                {
                    Ok(value.into())
                } else {
                    Ok(().into())
                }
            }
            Request::BlockExpiration {
                name,
                default: _,
                remove,
                value,
            } => {
                let value = value.map(Duration::from_secs);
                let value = if remove { Some(None) } else { value.map(Some) };

                if let Some(name) = name {
                    let holder = self.state.repositories.find(&name)?;

                    if let Some(value) = value {
                        holder.repository.set_block_expiration(value).await?;
                        Ok(().into())
                    } else {
                        let block_expiration = holder.repository.block_expiration().await;
                        Ok(Response::BlockExpiration(block_expiration))
                    }
                } else if let Some(value) = value {
                    ouisync_bridge::repository::set_default_block_expiration(
                        &self.state.config,
                        value,
                    )
                    .await?;
                    Ok(().into())
                } else {
                    let block_expiration =
                        ouisync_bridge::repository::get_default_block_expiration(
                            &self.state.config,
                        )
                        .await?;
                    Ok(Response::BlockExpiration(block_expiration))
                }
            }
        }
    }
}

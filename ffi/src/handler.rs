use crate::{
    directory,
    error::Error,
    file, network,
    protocol::{Request, Response},
    repository, share_token,
    state::State,
    state_monitor,
};
use async_trait::async_trait;
use ouisync_bridge::transport::SessionContext;
use ouisync_lib::{crypto::cipher::SecretKey, PeerAddr};
use std::{net::SocketAddr, sync::Arc};

#[derive(Clone)]
pub(crate) struct Handler {
    state: Arc<State>,
}

impl Handler {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ouisync_bridge::transport::Handler for Handler {
    type Request = Request;
    type Response = Response;
    type Error = Error;

    async fn handle(
        &self,
        request: Self::Request,
        context: &SessionContext,
    ) -> Result<Self::Response, Self::Error> {
        tracing::trace!(?request);

        let response = match request {
            Request::RepositoryCreate {
                path,
                read_secret,
                write_secret,
                share_token,
            } => repository::create(
                &self.state,
                path.into_std_path_buf(),
                read_secret,
                write_secret,
                share_token,
            )
            .await?
            .into(),
            Request::RepositoryOpen { path, secret } => {
                repository::open(&self.state, path.into_std_path_buf(), secret)
                    .await?
                    .into()
            }
            Request::RepositoryClose(handle) => {
                repository::close(&self.state, handle).await?.into()
            }
            Request::RepositorySubscribe(handle) => {
                repository::subscribe(&self.state, &context.notification_tx, handle)?.into()
            }
            Request::RepositorySetAccess {
                repository,
                read,
                write,
            } => self
                .state
                .repositories
                .get(repository)?
                .repository
                .set_access(read, write)
                .await?
                .into(),
            Request::RepositoryCredentials(handle) => {
                repository::credentials(&self.state, handle)?.into()
            }
            Request::RepositorySetCredentials {
                repository,
                credentials,
            } => repository::set_credentials(&self.state, repository, credentials)
                .await?
                .into(),
            Request::RepositorySetAccessMode {
                repository,
                access_mode,
                secret,
            } => {
                repository::set_access_mode(&self.state, repository, access_mode, secret).await?;
                ().into()
            }
            Request::RepositoryRequiresLocalSecretForReading(handle) => self
                .state
                .repositories
                .get(handle)?
                .repository
                .requires_local_secret_for_reading()
                .await?
                .into(),
            Request::RepositoryRequiresLocalSecretForWriting(handle) => self
                .state
                .repositories
                .get(handle)?
                .repository
                .requires_local_secret_for_writing()
                .await?
                .into(),
            Request::RepositoryInfoHash(handle) => {
                repository::info_hash(&self.state, handle)?.into()
            }
            Request::RepositoryDatabaseId(handle) => {
                repository::database_id(&self.state, handle).await?.into()
            }
            Request::RepositoryEntryType { repository, path } => {
                repository::entry_type(&self.state, repository, path)
                    .await?
                    .into()
            }
            Request::RepositoryMoveEntry {
                repository,
                src,
                dst,
            } => repository::move_entry(&self.state, repository, src, dst)
                .await?
                .into(),
            Request::RepositoryIsDhtEnabled(repository) => {
                repository::is_dht_enabled(&self.state, repository)?.into()
            }
            Request::RepositorySetDhtEnabled {
                repository,
                enabled,
            } => {
                repository::set_dht_enabled(&self.state, repository, enabled).await?;
                ().into()
            }
            Request::RepositoryIsPexEnabled(repository) => {
                repository::is_pex_enabled(&self.state, repository)?.into()
            }
            Request::RepositorySetPexEnabled {
                repository,
                enabled,
            } => {
                repository::set_pex_enabled(&self.state, repository, enabled).await?;
                ().into()
            }
            Request::RepositoryCreateShareToken {
                repository,
                secret,
                access_mode,
                name,
            } => repository::create_share_token(&self.state, repository, secret, access_mode, name)
                .await?
                .into(),
            Request::RepositoryCreateMirror { repository } => {
                repository::create_mirror(&self.state, repository)
                    .await?
                    .into()
            }
            Request::RepositoryDeleteMirror { repository } => {
                repository::delete_mirror(&self.state, repository)
                    .await?
                    .into()
            }
            Request::RepositoryMirrorExists { repository } => {
                repository::mirror_exists(&self.state, repository)
                    .await?
                    .into()
            }
            Request::ShareTokenMode(token) => share_token::mode(token).into(),
            Request::ShareTokenInfoHash(token) => share_token::info_hash(token).into(),
            Request::ShareTokenSuggestedName(token) => share_token::suggested_name(token).into(),
            Request::ShareTokenNormalize(token) => token.to_string().into(),
            Request::RepositoryAccessMode(repository) => {
                repository::access_mode(&self.state, repository)?.into()
            }
            Request::RepositorySyncProgress(repository) => {
                repository::sync_progress(&self.state, repository)
                    .await?
                    .into()
            }
            Request::RepositoryMountAll(mount_point) => {
                repository::mount_all(&self.state, mount_point)
                    .await?
                    .into()
            }
            Request::DirectoryCreate { repository, path } => {
                directory::create(&self.state, repository, path)
                    .await?
                    .into()
            }
            Request::DirectoryOpen { repository, path } => {
                directory::open(&self.state, repository, path).await?.into()
            }
            Request::DirectoryRemove {
                repository,
                path,
                recursive,
            } => directory::remove(&self.state, repository, path, recursive)
                .await?
                .into(),
            Request::FileOpen { repository, path } => {
                file::open(&self.state, repository, path).await?.into()
            }
            Request::FileCreate { repository, path } => {
                file::create(&self.state, repository, path).await?.into()
            }
            Request::FileRemove { repository, path } => {
                file::remove(&self.state, repository, path).await?.into()
            }
            Request::FileRead { file, offset, len } => {
                file::read(&self.state, file, offset, len).await?.into()
            }
            Request::FileWrite { file, offset, data } => {
                file::write(&self.state, file, offset, data).await?.into()
            }
            Request::FileTruncate { file, len } => {
                file::truncate(&self.state, file, len).await?.into()
            }
            Request::FileLen(file) => file::len(&self.state, file).await?.into(),
            Request::FileProgress(file) => file::progress(&self.state, file).await?.into(),
            Request::FileFlush(file) => file::flush(&self.state, file).await?.into(),
            Request::FileClose(file) => file::close(&self.state, file).await?.into(),
            Request::NetworkInit(defaults) => {
                ouisync_bridge::network::init(&self.state.network, &self.state.config, defaults)
                    .await;
                ().into()
            }
            Request::NetworkSubscribe => {
                network::subscribe(&self.state, &context.notification_tx).into()
            }
            Request::NetworkBind {
                quic_v4,
                quic_v6,
                tcp_v4,
                tcp_v6,
            } => {
                ouisync_bridge::network::bind(
                    &self.state.network,
                    &self.state.config,
                    &[
                        quic_v4.map(SocketAddr::from).map(PeerAddr::Quic),
                        quic_v6.map(SocketAddr::from).map(PeerAddr::Quic),
                        tcp_v4.map(SocketAddr::from).map(PeerAddr::Tcp),
                        tcp_v6.map(SocketAddr::from).map(PeerAddr::Tcp),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>(),
                )
                .await;
                ().into()
            }
            Request::NetworkTcpListenerLocalAddrV4 => self
                .state
                .network
                .listener_local_addrs()
                .into_iter()
                .find(|addr| matches!(addr, PeerAddr::Tcp(SocketAddr::V4(_))))
                .map(|addr| *addr.socket_addr())
                .into(),
            Request::NetworkTcpListenerLocalAddrV6 => self
                .state
                .network
                .listener_local_addrs()
                .into_iter()
                .find(|addr| matches!(addr, PeerAddr::Tcp(SocketAddr::V6(_))))
                .map(|addr| *addr.socket_addr())
                .into(),
            Request::NetworkQuicListenerLocalAddrV4 => self
                .state
                .network
                .listener_local_addrs()
                .into_iter()
                .find(|addr| matches!(addr, PeerAddr::Quic(SocketAddr::V4(_))))
                .map(|addr| *addr.socket_addr())
                .into(),
            Request::NetworkQuicListenerLocalAddrV6 => self
                .state
                .network
                .listener_local_addrs()
                .into_iter()
                .find(|addr| matches!(addr, PeerAddr::Quic(SocketAddr::V6(_))))
                .map(|addr| *addr.socket_addr())
                .into(),
            Request::NetworkAddUserProvidedPeer(addr) => {
                ouisync_bridge::network::add_user_provided_peers(
                    &self.state.network,
                    &self.state.config,
                    &[addr],
                )
                .await;
                ().into()
            }
            Request::NetworkRemoveUserProvidedPeer(addr) => {
                ouisync_bridge::network::remove_user_provided_peers(
                    &self.state.network,
                    &self.state.config,
                    &[addr],
                )
                .await;
                ().into()
            }
            Request::NetworkUserProvidedPeers => {
                ouisync_bridge::network::user_provided_peers(&self.state.config)
                    .await
                    .into()
            }
            Request::NetworkKnownPeers => self.state.network.peer_info_collector().collect().into(),
            Request::NetworkThisRuntimeId => network::this_runtime_id(&self.state).into(),
            Request::NetworkCurrentProtocolVersion => {
                self.state.network.current_protocol_version().into()
            }
            Request::NetworkHighestSeenProtocolVersion => {
                self.state.network.highest_seen_protocol_version().into()
            }
            Request::NetworkIsPortForwardingEnabled => {
                self.state.network.is_port_forwarding_enabled().into()
            }
            Request::NetworkSetPortForwardingEnabled(enabled) => {
                ouisync_bridge::network::set_port_forwarding_enabled(
                    &self.state.network,
                    &self.state.config,
                    enabled,
                )
                .await;
                ().into()
            }
            Request::NetworkIsLocalDiscoveryEnabled => {
                self.state.network.is_local_discovery_enabled().into()
            }
            Request::NetworkSetLocalDiscoveryEnabled(enabled) => {
                ouisync_bridge::network::set_local_discovery_enabled(
                    &self.state.network,
                    &self.state.config,
                    enabled,
                )
                .await;
                ().into()
            }
            Request::NetworkAddCacheServer(host) => {
                // NOTE: Do not inline this into the `if`, otherwise we would hold the lock across
                // await.
                let new = self
                    .state
                    .cache_servers
                    .lock()
                    .unwrap()
                    .insert(host.clone());

                if new {
                    ouisync_bridge::network::add_cache_server(&self.state.network, &host).await?;
                }

                ().into()
            }
            Request::NetworkExternalAddrV4 => self.state.network.external_addr_v4().await.into(),
            Request::NetworkExternalAddrV6 => self.state.network.external_addr_v6().await.into(),
            Request::NetworkNatBehavior => self.state.network.nat_behavior().await.into(),
            Request::NetworkTrafficStats => self.state.network.traffic_stats().into(),
            Request::NetworkShutdown => {
                self.state.network.shutdown().await;
                ().into()
            }
            Request::StateMonitorGet(path) => state_monitor::get(&self.state, path)?.into(),
            Request::StateMonitorSubscribe(path) => {
                state_monitor::subscribe(&self.state, &context.notification_tx, path)?.into()
            }
            Request::Unsubscribe(handle) => {
                self.state.remove_task(handle);
                ().into()
            }
            Request::GenerateSaltForSecretKey => SecretKey::random_salt().as_ref().to_vec().into(),
            Request::DeriveSecretKey { password, salt } => {
                // TODO: This is a slow operation, do we need to send it to the thread pool?
                SecretKey::derive_from_password(&password, &salt)
                    .as_array()
                    .to_vec()
                    .into()
            }
            Request::GetReadPasswordSalt(handle) => self
                .state
                .repositories
                .get(handle)?
                .repository
                .get_read_password_salt()
                .await?
                .as_array()
                .to_vec()
                .into(),
            Request::GetWritePasswordSalt(handle) => self
                .state
                .repositories
                .get(handle)?
                .repository
                .get_write_password_salt()
                .await?
                .as_array()
                .to_vec()
                .into(),
        };

        Ok(response)
    }
}

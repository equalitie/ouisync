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
use ouisync_bridge::transport::NotificationSender;
use ouisync_lib::PeerAddr;
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
        notification_tx: &NotificationSender,
    ) -> Result<Self::Response, Self::Error> {
        // DEBUG
        tracing::debug!(?request);

        let response = match request {
            Request::RepositoryCreate {
                path,
                read_password,
                write_password,
                share_token,
            } => repository::create(
                &self.state,
                path.into_std_path_buf(),
                read_password,
                write_password,
                share_token,
            )
            .await?
            .into(),
            Request::RepositoryOpen { path, password } => {
                repository::open(&self.state, path.into_std_path_buf(), password)
                    .await?
                    .into()
            }
            Request::RepositoryClose(handle) => {
                repository::close(&self.state, handle).await?.into()
            }
            Request::RepositoryCreateReopenToken(handle) => {
                repository::create_reopen_token(&self.state, handle).into()
            }
            Request::RepositoryReopen { path, token } => {
                repository::reopen(&self.state, path.into_std_path_buf(), token)
                    .await?
                    .into()
            }
            Request::RepositorySubscribe(handle) => {
                repository::subscribe(&self.state, notification_tx, handle).into()
            }
            Request::RepositorySetReadAccess {
                repository,
                password,
                share_token,
            } => repository::set_read_access(&self.state, repository, password, share_token)
                .await?
                .into(),
            Request::RepositorySetReadAndWriteAccess {
                repository,
                old_password,
                new_password,
                share_token,
            } => repository::set_read_and_write_access(
                &self.state,
                repository,
                old_password,
                new_password,
                share_token,
            )
            .await?
            .into(),
            Request::RepositoryRemoveReadKey(handle) => {
                repository::remove_read_key(&self.state, handle)
                    .await?
                    .into()
            }
            Request::RepositoryRemoveWriteKey(handle) => {
                repository::remove_write_key(&self.state, handle)
                    .await?
                    .into()
            }
            Request::RepositoryRequiresLocalPasswordForReading(handle) => {
                repository::requires_local_password_for_reading(&self.state, handle)
                    .await?
                    .into()
            }
            Request::RepositoryRequiresLocalPasswordForWriting(handle) => {
                repository::requires_local_password_for_writing(&self.state, handle)
                    .await?
                    .into()
            }
            Request::RepositoryInfoHash(handle) => {
                repository::info_hash(&self.state, handle).into()
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
                repository::is_dht_enabled(&self.state, repository).into()
            }
            Request::RepositorySetDhtEnabled {
                repository,
                enabled,
            } => {
                repository::set_dht_enabled(&self.state, repository, enabled).await;
                ().into()
            }
            Request::RepositoryIsPexEnabled(repository) => {
                repository::is_pex_enabled(&self.state, repository).into()
            }
            Request::RepositorySetPexEnabled {
                repository,
                enabled,
            } => {
                repository::set_pex_enabled(&self.state, repository, enabled).await;
                ().into()
            }
            Request::RepositoryCreateShareToken {
                repository,
                password,
                access_mode,
                name,
            } => {
                repository::create_share_token(&self.state, repository, password, access_mode, name)
                    .await?
                    .into()
            }
            Request::RepositoryMirror { repository } => {
                repository::mirror(&self.state, repository).await?.into()
            }
            Request::ShareTokenMode(token) => share_token::mode(token).into(),
            Request::ShareTokenInfoHash(token) => share_token::info_hash(token).into(),
            Request::ShareTokenSuggestedName(token) => share_token::suggested_name(token).into(),
            Request::ShareTokenNormalize(token) => token.to_string().into(),
            Request::RepositoryAccessMode(repository) => {
                repository::access_mode(&self.state, repository).into()
            }
            Request::RepositorySyncProgress(repository) => {
                repository::sync_progress(&self.state, repository)
                    .await?
                    .into()
            }
            Request::RepositoryMountAll(mount_point) => {
                self.state.mount_all(mount_point).await?.into()
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
            Request::FileLen(file) => file::len(&self.state, file).await.into(),
            Request::FileProgress(file) => file::progress(&self.state, file).await?.into(),
            Request::FileFlush(file) => file::flush(&self.state, file).await?.into(),
            Request::FileClose(file) => file::close(&self.state, file).await?.into(),
            Request::NetworkInit(defaults) => {
                ouisync_bridge::network::init(&self.state.network, &self.state.config, defaults)
                    .await;
                ().into()
            }
            Request::NetworkSubscribe => network::subscribe(&self.state, notification_tx).into(),
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
            Request::NetworkAddStorageServer(host) => {
                ouisync_bridge::network::add_storage_server(&self.state.network, &host).await?;
                self.state.storage_servers.lock().unwrap().insert(host);
                ().into()
            }
            Request::NetworkShutdown => {
                self.state.network.shutdown().await;
                ().into()
            }
            Request::StateMonitorGet(path) => state_monitor::get(&self.state, path)?.into(),
            Request::StateMonitorSubscribe(path) => {
                state_monitor::subscribe(&self.state, notification_tx, path)?.into()
            }
            Request::Unsubscribe(handle) => {
                self.state.unsubscribe(handle);
                ().into()
            }
        };

        Ok(response)
    }
}

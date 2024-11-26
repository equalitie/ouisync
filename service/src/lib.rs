pub mod protocol;
pub mod transport;

mod error;
mod metrics;
mod repository;
mod state;
mod utils;

pub use error::Error;

use futures_util::SinkExt;
use metrics::MetricsServer;
use ouisync::PeerAddr;
use protocol::{DecodeError, Message, ProtocolError, Request, Response, ServerPayload};
use slab::Slab;
use state::State;
use std::{path::PathBuf, time::Duration};
use tokio::{
    select,
    time::{self, MissedTickBehavior},
};
use tokio_stream::{StreamExt, StreamMap, StreamNotifyClose};
use transport::{local::LocalServer, ReadError, ServerReader, ServerWriter};

const REPOSITORY_EXPIRATION_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);

pub struct Defaults {
    pub store_dir: PathBuf,
    pub mount_dir: PathBuf,
    pub bind: Vec<PeerAddr>,
    pub local_discovery_enabled: bool,
    pub port_forwarding_enabled: bool,
}

pub struct Service {
    state: State,
    local_server: LocalServer,
    readers: StreamMap<ConnectionId, StreamNotifyClose<ServerReader>>,
    writers: Slab<ServerWriter>,
    metrics_server: MetricsServer,
}

impl Service {
    pub async fn init(
        local_socket_path: PathBuf,
        config_dir: PathBuf,
        defaults: Defaults,
    ) -> Result<Self, Error> {
        let state = State::init(config_dir, defaults).await?;
        let local_server = LocalServer::bind(&local_socket_path).await?;

        let metrics_server = MetricsServer::init(&state).await?;

        Ok(Self {
            state,
            local_server,
            readers: StreamMap::new(),
            writers: Slab::new(),
            metrics_server,
        })
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        let mut repo_expiration_interval = time::interval(REPOSITORY_EXPIRATION_POLL_INTERVAL);
        repo_expiration_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            select! {
                result = self.local_server.accept() => {
                    let (reader, writer) = result?;
                    self.insert_connection(
                        ServerReader::Local(reader),
                        ServerWriter::Local(writer)
                    );
                }
                Some((conn_id, message)) = self.readers.next() => {
                    if let Some(message) = message {
                        self.handle_message(conn_id, message).await
                    } else {
                        self.remove_connection(conn_id)
                    }
                }
                _ = repo_expiration_interval.tick() => {
                    self.state.delete_expired_repositories().await;
                }
            }
        }
    }

    pub async fn close(&mut self) {
        self.metrics_server.close();

        self.readers.clear();

        for mut writer in self.writers.drain() {
            if let Err(error) = writer.close().await {
                tracing::warn!(?error, "failed to close local connection");
            }
        }

        self.state.close().await;
    }

    async fn handle_message(
        &mut self,
        conn_id: ConnectionId,
        message: Result<Message<Request>, ReadError>,
    ) {
        match message {
            Ok(message) => {
                let id = message.id;
                let payload = self.dispatch_message(conn_id, message).await.into();
                let message = Message { id, payload };

                self.send_message(conn_id, message).await;
            }
            Err(ReadError::Receive(error)) => {
                tracing::error!(?error, "failed to receive message");
                self.remove_connection(conn_id);
            }
            Err(ReadError::Decode(DecodeError::Id)) => {
                tracing::error!("failed to decode message id");
                self.remove_connection(conn_id);
            }
            Err(ReadError::Decode(DecodeError::Payload(id, error))) => {
                tracing::warn!(?error, ?id, "failed to decode message payload");

                let message = Message {
                    id,
                    payload: ServerPayload::Failure(error.into()),
                };
                self.send_message(conn_id, message).await;
            }
        }
    }

    async fn dispatch_message(
        &mut self,
        _conn_id: ConnectionId,
        message: Message<Request>,
    ) -> Result<Response, ProtocolError> {
        tracing::trace!(?message, "received");

        match message.payload {
            Request::MetricsBind(addr) => {
                Ok(self.metrics_server.bind(&self.state, addr).await?.into())
            }
            Request::MetricsGetListenerAddr => todo!(),
            Request::NetworkAddUserProvidedPeers(addrs) => {
                self.state.add_user_provided_peers(addrs).await;
                Ok(().into())
            }
            Request::NetworkBind(addrs) => {
                self.state.bind_network(addrs).await;
                Ok(().into())
            }
            Request::NetworkGetListenerAddrs => {
                Ok(self.state.network.listener_local_addrs().into())
            }
            Request::NetworkGetPeers => {
                Ok(self.state.network.peer_info_collector().collect().into())
            }
            Request::NetworkGetUserProvidedPeers => {
                Ok(self.state.user_provided_peers().await.into())
            }
            Request::NetworkIsLocalDiscoveryEnabled => {
                Ok(self.state.network.is_local_discovery_enabled().into())
            }
            Request::NetworkIsPexRecvEnabled => Ok(self.state.network.is_pex_recv_enabled().into()),
            Request::NetworkIsPexSendEnabled => Ok(self.state.network.is_pex_send_enabled().into()),
            Request::NetworkIsPortForwardingEnabled => {
                Ok(self.state.network.is_port_forwarding_enabled().into())
            }
            Request::NetworkRemoveUserProvidedPeers(addrs) => {
                self.state.remove_user_provided_peers(addrs).await;
                Ok(().into())
            }
            Request::NetworkSetLocalDiscoveryEnabled(enabled) => {
                self.state.set_local_discovery_enabled(enabled).await;
                Ok(().into())
            }
            Request::NetworkSetPexRecvEnabled(enabled) => {
                self.state.set_pex_recv_enabled(enabled).await;
                Ok(().into())
            }
            Request::NetworkSetPexSendEnabled(enabled) => {
                self.state.set_pex_send_enabled(enabled).await;
                Ok(().into())
            }
            Request::NetworkSetPortForwardingEnabled(enabled) => {
                self.state.set_port_forwarding_enabled(enabled).await;
                Ok(().into())
            }
            Request::RemoteControlBind(_addr) => todo!(),
            Request::RemoteControlGetListenerAddr => todo!(),
            Request::RepositoryCreate {
                name,
                read_secret,
                write_secret,
                token,
            } => {
                let handle = self
                    .state
                    .create_repository(name, read_secret, write_secret, token)
                    .await?;

                Ok(handle.into())
            }
            Request::RepositoryDelete(handle) => {
                self.state.delete_repository(handle).await?;
                Ok(().into())
            }
            Request::RepositoryExport { handle, output } => {
                let output = self.state.export_repository(handle, output).await?;
                Ok(output.into())
            }
            Request::RepositoryFind(name) => Ok(self.state.find_repository(&name)?.into()),
            Request::RepositoryGetBlockExpiration(handle) => {
                Ok(self.state.block_expiration(handle)?.into())
            }
            Request::RepositoryGetDefaultBlockExpiration => {
                Ok(self.state.default_block_expiration().await?.into())
            }
            Request::RepositoryGetDefaultRepositoryExpiration => {
                Ok(self.state.default_repository_expiration().await?.into())
            }
            Request::RepositoryGetMountDir => Ok(self.state.mount_dir().into()),
            Request::RepositoryGetQuota(handle) => {
                Ok(self.state.repository_quota(handle).await?.into())
            }
            Request::RepositoryGetRepositoryExpiration(handle) => {
                Ok(self.state.repository_expiration(handle).await?.into())
            }
            Request::RepositoryGetStoreDir => Ok(self.state.store_dir().into()),
            Request::RepositoryGetDefaultQuota => Ok(self.state.default_quota().await?.into()),
            Request::RepositoryImport {
                input,
                name,
                mode,
                force,
            } => {
                let handle = self
                    .state
                    .import_repository(input, name, mode, force)
                    .await?;
                Ok(handle.into())
            }
            Request::RepositoryIsDhtEnabled(handle) => {
                Ok(self.state.is_repository_dht_enabled(handle)?.into())
            }
            Request::RepositoryIsPexEnabled(handle) => {
                Ok(self.state.is_repository_pex_enabled(handle)?.into())
            }
            Request::RepositoryList => Ok(self.state.list_repositories().into()),
            Request::RepositoryMount(handle) => {
                Ok(self.state.mount_repository(handle).await?.into())
            }
            Request::RepositoryResetAccess { handle, token } => {
                self.state.reset_repository_access(handle, token).await?;
                Ok(().into())
            }
            Request::RepositorySetBlockExpiration { handle, value } => {
                self.state.set_block_expiration(handle, value).await?;
                Ok(().into())
            }
            Request::RepositorySetDefaultBlockExpiration { value } => {
                self.state.set_default_block_expiration(value).await?;
                Ok(().into())
            }
            Request::RepositorySetDefaultRepositoryExpiration { value } => {
                self.state.set_default_repository_expiration(value).await?;
                Ok(().into())
            }
            Request::RepositorySetDefaultQuota { quota } => {
                self.state.set_default_quota(quota).await?;
                Ok(().into())
            }
            Request::RepositorySetDhtEnabled { handle, enabled } => {
                self.state
                    .set_repository_dht_enabled(handle, enabled)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetMountDir(path) => {
                self.state.set_mount_dir(path).await?;
                Ok(().into())
            }
            Request::RepositorySetPexEnabled { handle, enabled } => {
                self.state
                    .set_repository_pex_enabled(handle, enabled)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetQuota { handle, quota } => {
                self.state.set_repository_quota(handle, quota).await?;
                Ok(().into())
            }
            Request::RepositorySetRepositoryExpiration { handle, value } => {
                self.state.set_repository_expiration(handle, value).await?;
                Ok(().into())
            }
            Request::RepositorySetStoreDir(path) => {
                self.state.set_store_dir(path).await?;
                Ok(().into())
            }
            Request::RepositoryShare {
                handle,
                secret,
                mode,
            } => Ok(self
                .state
                .share_repository(handle, secret, mode)
                .await?
                .into()),
            Request::RepositoryUnmount(handle) => {
                self.state.unmount_repository(handle).await?;
                Ok(().into())
            }
        }
    }

    async fn send_message(&mut self, conn_id: ConnectionId, message: Message<ServerPayload>) {
        let Some(writer) = self.writers.get_mut(conn_id) else {
            tracing::error!("connection not found");
            return;
        };

        tracing::trace!(?message, "sending");

        match writer.send(message).await {
            Ok(()) => (),
            Err(error) => {
                tracing::error!(?error, "failed to send message");
                self.remove_connection(conn_id);
            }
        }
    }

    fn insert_connection(&mut self, reader: ServerReader, writer: ServerWriter) {
        let conn_id = self.writers.insert(writer);
        self.readers.insert(conn_id, StreamNotifyClose::new(reader));
    }

    fn remove_connection(&mut self, conn_id: ConnectionId) {
        self.readers.remove(&conn_id);
        self.writers.try_remove(conn_id);
    }
}

type ConnectionId = usize;

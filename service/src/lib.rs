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
use protocol::{DecodeError, Message, ProtocolError, Request, Response, ServerPayload};
use slab::Slab;
use state::State;
use std::path::PathBuf;
use tokio::select;
use tokio_stream::{StreamExt, StreamMap, StreamNotifyClose};
use tracing::instrument;
use transport::{LocalServer, LocalServerReader, LocalServerWriter, ReadError};

pub struct Service {
    state: State,
    local_server: LocalServer,
    local_readers: StreamMap<ConnectionId, StreamNotifyClose<LocalServerReader>>,
    local_writers: Slab<LocalServerWriter>,
    metrics_server: MetricsServer,
}

impl Service {
    pub async fn init(local_socket_path: PathBuf, config_dir: PathBuf) -> Result<Self, Error> {
        let state = State::init(&config_dir).await?;
        let local_server = LocalServer::bind(&local_socket_path).await?;

        let metrics_server = MetricsServer::init(&state).await?;

        Ok(Self {
            state,
            local_server,
            local_readers: StreamMap::new(),
            local_writers: Slab::new(),
            metrics_server,
        })
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        loop {
            select! {
                result = self.local_server.accept() => {
                    let (reader, writer) = result?;
                    self.insert_local_connection(reader, writer);
                }
                Some((conn_id, message)) = self.local_readers.next() => {
                    if let Some(message) = message {
                        self.handle_message(conn_id, message).await
                    } else {
                        self.remove_local_connection(conn_id)
                    }
                }
            }
        }
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        self.metrics_server.close();
        self.state.close().await?;

        todo!()
    }

    #[instrument(skip(self))]
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
                self.remove_local_connection(conn_id);
            }
            Err(ReadError::Decode(DecodeError::Id(error))) => {
                tracing::error!(?error, "failed to decode message id");
                self.remove_local_connection(conn_id);
            }
            Err(ReadError::Decode(DecodeError::Payload(id, error))) => {
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
        conn_id: ConnectionId,
        message: Message<Request>,
    ) -> Result<Response, ProtocolError> {
        match message.payload {
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
            Request::MetricsBind { addr } => {
                Ok(self.metrics_server.bind(&self.state, addr).await?.into())
            }
            Request::RemoteControlBind { addrs: _ } => todo!(),
            Request::RepositoryGetMountDir => Ok(self
                .state
                .mount_dir()
                .ok_or(Error::MountDirUnspecified)?
                .into()),
            Request::RepositoryGetQuota(handle) => {
                Ok(self.state.repository_quota(handle).await?.into())
            }
            Request::RepositoryGetStoreDir => Ok(self
                .state
                .store_dir()
                .ok_or(Error::StoreDirUnspecified)?
                .into()),
            Request::RepositoryCreate {
                name,
                read_secret,
                write_secret,
                share_token,
            } => {
                let handle = self
                    .state
                    .create_repository(name, read_secret, write_secret, share_token)
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
        let Some(writer) = self.local_writers.get_mut(conn_id) else {
            tracing::error!("connection not found");
            return;
        };

        match writer.send(message).await {
            Ok(()) => (),
            Err(error) => {
                tracing::error!(?error, "failed to send message");
                self.remove_local_connection(conn_id);
            }
        }
    }

    fn insert_local_connection(&mut self, reader: LocalServerReader, writer: LocalServerWriter) {
        let conn_id = self.local_writers.insert(writer);
        self.local_readers
            .insert(conn_id, StreamNotifyClose::new(reader));
    }

    fn remove_local_connection(&mut self, conn_id: ConnectionId) {
        self.local_readers.remove(&conn_id);
        self.local_writers.try_remove(conn_id);
    }
}

type ConnectionId = usize;

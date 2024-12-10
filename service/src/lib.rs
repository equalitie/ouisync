pub mod ffi;
pub mod protocol;
pub mod transport;

mod error;
mod file;
mod metrics;
mod repository;
mod state;
mod subscription;
mod utils;

#[cfg(test)]
mod test_utils;

pub use error::Error;

use futures_util::SinkExt;
use metrics::MetricsServer;
use ouisync::crypto::{cipher::SecretKey, PasswordSalt};
use ouisync_bridge::config::{ConfigError, ConfigKey};
use protocol::{DecodeError, Message, MessageId, ProtocolError, Request, Response, ResponseResult};
use rand::{rngs::OsRng, Rng};
use slab::Slab;
use state::State;
use std::{
    convert::Infallible,
    future, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use subscription::SubscriptionStream;
use tokio::{
    select,
    time::{self, MissedTickBehavior},
};
use tokio_stream::{StreamExt, StreamMap, StreamNotifyClose};
use transport::{
    local::LocalServer,
    remote::{RemoteServer, RemoteServerReader, RemoteServerWriter},
    ReadError, ServerReader, ServerWriter,
};

const REPOSITORY_EXPIRATION_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);

const REMOTE_CONTROL_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("remote_control", "Remote control endpoint address");

pub struct Service {
    state: State,
    local_server: LocalServer,
    remote_server: Option<RemoteServer>,
    readers: StreamMap<ConnectionId, StreamNotifyClose<ServerReader>>,
    writers: Slab<ServerWriter>,
    subscriptions: StreamMap<SubscriptionId, SubscriptionStream>,
    metrics_server: MetricsServer,
}

impl Service {
    pub async fn init(local_socket_path: PathBuf, config_dir: PathBuf) -> Result<Self, Error> {
        let state = State::init(config_dir).await?;
        let local_server =
            LocalServer::bind(&local_socket_path)
                .await
                .map_err(|error| match error.kind() {
                    io::ErrorKind::AddrInUse => Error::ServiceAlreadyRunning,
                    _ => {
                        tracing::error!(
                            ?error,
                            "failed to bind local listener to {:?}",
                            local_socket_path,
                        );
                        Error::Bind(error)
                    }
                })?;

        let remote_server = match state.config.entry(REMOTE_CONTROL_KEY).get().await {
            Ok(addr) => Some(
                RemoteServer::bind(addr, state.remote_server_config().await?)
                    .await
                    .map_err(Error::Bind)?,
            ),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        let metrics_server = MetricsServer::init(&state).await?;

        Ok(Self {
            state,
            local_server,
            remote_server,
            readers: StreamMap::new(),
            writers: Slab::new(),
            subscriptions: StreamMap::new(),
            metrics_server,
        })
    }

    /// Runs the service. The future returned from this function never completes (unless it errors)
    /// but it's safe to cancel.
    //
    // Note we are using `Infallible` for the `Ok` variant which reads a bit weird but it just means
    // the function never returns `Ok`. When `!` (the "never" type) is stabilized we should use
    // that instead.
    pub async fn run(&mut self) -> Result<Infallible, Error> {
        let mut repo_expiration_interval = time::interval(REPOSITORY_EXPIRATION_POLL_INTERVAL);
        repo_expiration_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            select! {
                result = self.local_server.accept() => {
                    let (reader, writer) = result.map_err(Error::Accept)?;
                    self.insert_connection(
                        ServerReader::Local(reader),
                        ServerWriter::Local(writer)
                    );
                }
                result = maybe_accept(self.remote_server.as_mut()) => {
                    let (reader, writer) = result.map_err(Error::Accept)?;
                    self.insert_connection(ServerReader::Remote(reader), ServerWriter::Remote(writer));
                }
                Some((conn_id, message)) = self.readers.next() => {
                    if let Some(message) = message {
                        self.handle_message(conn_id, message).await
                    } else {
                        self.remove_connection(conn_id)
                    }
                }
                Some(((conn_id, message_id), response)) = self.subscriptions.next() => {
                    self.send_message(
                        conn_id,
                        Message {
                            id: message_id,
                            payload: ResponseResult::Success(response)
                        },
                    ).await;
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

    pub fn store_dir(&self) -> Option<&Path> {
        self.state.store_dir()
    }

    pub async fn set_store_dir(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        self.state.set_store_dir(path.into()).await
    }

    pub(crate) async fn bind_remote_control(
        &mut self,
        addr: Option<SocketAddr>,
    ) -> Result<u16, Error> {
        if let Some(addr) = addr {
            let config = self.state.remote_server_config().await?;
            let remote_server = RemoteServer::bind(addr, config)
                .await
                .map_err(Error::Bind)?;
            let port = remote_server.local_addr().port();

            self.remote_server = Some(remote_server);

            Ok(port)
        } else {
            self.remote_server = None;

            Ok(0)
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    #[cfg(test)]
    pub(crate) fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    async fn handle_message(
        &mut self,
        conn_id: ConnectionId,
        message: Result<Message<Request>, ReadError>,
    ) {
        match message {
            Ok(message) => {
                let id = message.id;
                let payload = self.dispatch_message(conn_id, message).await;
                let message = Message {
                    id,
                    payload: payload.into(),
                };

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

                self.send_message(
                    conn_id,
                    Message {
                        id,
                        payload: ResponseResult::Failure(error.into()),
                    },
                )
                .await;
            }
            Err(ReadError::Validate(id, error)) => {
                tracing::warn!(?error, ?id, "failed to validate message");

                self.send_message(
                    conn_id,
                    Message {
                        id,
                        payload: ResponseResult::Failure(error.into()),
                    },
                )
                .await;
            }
        }
    }

    async fn dispatch_message(
        &mut self,
        conn_id: ConnectionId,
        message: Message<Request>,
    ) -> Result<Response, ProtocolError> {
        tracing::trace!(?message, "received");

        match message.payload {
            Request::DirectoryCreate { repository, path } => {
                self.state.create_directory(repository, path).await?;
                Ok(().into())
            }
            Request::DirectoryRead { repository, path } => {
                Ok(self.state.read_directory(repository, path).await?.into())
            }
            Request::DirectoryRemove {
                repository,
                path,
                recursive,
            } => {
                self.state
                    .remove_directory(repository, path, recursive)
                    .await?;
                Ok(().into())
            }
            Request::FileClose(file) => {
                self.state.close_file(file).await?;
                Ok(().into())
            }
            Request::FileCreate { repository, path } => {
                Ok(self.state.create_file(repository, path).await?.into())
            }
            Request::FileExists { repository, path } => {
                Ok(self.state.file_exists(repository, path).await?.into())
            }
            Request::FileFlush(file) => {
                self.state.flush_file(file).await?;
                Ok(().into())
            }
            Request::FileLen(file) => Ok(self.state.file_len(file)?.into()),
            Request::FileOpen { repository, path } => {
                Ok(self.state.open_file(repository, path).await?.into())
            }
            Request::FileProgress(file) => Ok(self.state.file_progress(file).await?.into()),
            Request::FileRead { file, offset, len } => {
                Ok(self.state.read_file(file, offset, len).await?.into())
            }
            Request::FileRemove { repository, path } => {
                self.state.remove_file(repository, path).await?;
                Ok(().into())
            }
            Request::FileTruncate { file, len } => {
                self.state.truncate_file(file, len).await?;
                Ok(().into())
            }
            Request::FileWrite { file, offset, data } => {
                self.state.write_file(file, offset, data.into()).await?;
                Ok(().into())
            }
            Request::PasswordGenerateSalt => {
                let salt: PasswordSalt = OsRng.gen();
                Ok(salt.as_ref().to_vec().into())
            }
            Request::PasswordDeriveSecretKey { password, salt } => {
                let secret_key = SecretKey::derive_from_password(&password, &salt);
                Ok(secret_key.as_array().to_vec().into())
            }
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
            Request::NetworkCurrentProtocolVersion => {
                Ok(self.state.network.current_protocol_version().into())
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
            Request::NetworkInit(defaults) => {
                self.state.init_network(defaults).await;
                Ok(().into())
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
            Request::NetworkStats => Ok(self.state.network.stats().into()),
            Request::NetworkSubscribe => {
                let rx = self.state.network.subscribe();
                self.subscriptions.insert((conn_id, message.id), rx.into());
                Ok(().into())
            }
            Request::RemoteControlBind(addr) => {
                self.bind_remote_control(addr).await?;
                Ok(().into())
            }
            Request::RemoteControlGetListenerAddr => Ok(self
                .remote_server
                .as_ref()
                .map(|server| server.local_addr())
                .into()),
            Request::RepositoryClose(repository) => {
                self.state.close_repository(repository).await?;
                Ok(().into())
            }
            Request::RepositoryCreate {
                name,
                read_secret,
                write_secret,
                token,
                dht,
                pex,
            } => {
                let handle = self
                    .state
                    .create_repository(name, read_secret, write_secret, token, dht, pex)
                    .await?;

                Ok(handle.into())
            }
            Request::RepositoryCreateMirror { repository, host } => {
                self.state
                    .create_repository_mirror(repository, host)
                    .await?;
                Ok(().into())
            }
            Request::RepositoryCredentials(repository) => {
                Ok(self.state.repository_credentials(repository)?.into())
            }
            Request::RepositoryDelete(handle) => {
                self.state.delete_repository(handle).await?;
                Ok(().into())
            }
            Request::RepositoryDeleteByName(name) => {
                let handle = self.state.find_repository(&name)?;
                self.state.delete_repository(handle).await?;
                Ok(().into())
            }
            Request::RepositoryDeleteMirror { repository, host } => {
                self.state
                    .delete_repository_mirror(repository, host)
                    .await?;
                Ok(().into())
            }
            Request::RepositoryEntryType { repository, path } => Ok(self
                .state
                .repository_entry_type(repository, path)
                .await?
                .into()),
            Request::RepositoryExport { repository, output } => {
                let output = self.state.export_repository(repository, output).await?;
                Ok(output.into())
            }
            Request::RepositoryFind(name) => Ok(self.state.find_repository(&name)?.into()),
            Request::RepositoryGetAccessMode(repository) => {
                Ok(self.state.repository_access_mode(repository)?.into())
            }
            Request::RepositoryGetBlockExpiration(repository) => {
                Ok(self.state.block_expiration(repository)?.into())
            }
            Request::RepositoryGetDefaultBlockExpiration => {
                Ok(self.state.default_block_expiration().await?.into())
            }
            Request::RepositoryGetDefaultRepositoryExpiration => {
                Ok(self.state.default_repository_expiration().await?.into())
            }
            Request::RepositoryGetMetadata { repository, key } => Ok(self
                .state
                .repository_metadata(repository, key)
                .await?
                .into()),
            Request::RepositoryGetMountDir => Ok(self.state.mount_dir().into()),
            Request::RepositoryGetQuota(repository) => {
                Ok(self.state.repository_quota(repository).await?.into())
            }
            Request::RepositoryGetRepositoryExpiration(repository) => {
                Ok(self.state.repository_expiration(repository).await?.into())
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
            Request::RepositoryIsDhtEnabled(repository) => {
                Ok(self.state.is_repository_dht_enabled(repository)?.into())
            }
            Request::RepositoryIsPexEnabled(repository) => {
                Ok(self.state.is_repository_pex_enabled(repository)?.into())
            }
            Request::RepositoryIsSyncEnabled(repository) => {
                Ok(self.state.is_repository_sync_enabled(repository)?.into())
            }
            Request::RepositoryList => Ok(self.state.list_repositories().into()),
            Request::RepositoryMirrorExists { repository, host } => Ok(self
                .state
                .repository_mirror_exists(repository, host)
                .await?
                .into()),
            Request::RepositoryMount(repository) => {
                Ok(self.state.mount_repository(repository).await?.into())
            }
            Request::RepositoryMoveEntry {
                repository,
                src,
                dst,
            } => {
                self.state
                    .move_repository_entry(repository, src, dst)
                    .await?;
                Ok(().into())
            }
            Request::RepositoryOpen { name, secret } => {
                Ok(self.state.open_repository(name, secret).await?.into())
            }
            Request::RepositoryResetAccess { repository, token } => {
                self.state
                    .reset_repository_access(repository, token)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetAccess {
                repository,
                read,
                write,
            } => {
                self.state
                    .set_repository_access(repository, read, write)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetAccessMode {
                repository,
                access_mode,
                secret,
            } => {
                self.state
                    .set_repository_access_mode(repository, access_mode, secret)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetBlockExpiration { repository, value } => {
                self.state.set_block_expiration(repository, value).await?;
                Ok(().into())
            }
            Request::RepositorySetCredentials {
                repository,
                credentials,
            } => {
                self.state
                    .set_repository_credentials(repository, credentials.into())
                    .await?;
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
            Request::RepositorySetDhtEnabled {
                repository,
                enabled,
            } => {
                self.state
                    .set_repository_dht_enabled(repository, enabled)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetMetadata { repository, edits } => Ok(self
                .state
                .set_repository_metadata(repository, edits)
                .await?
                .into()),
            Request::RepositorySetMountDir(path) => {
                self.state.set_mount_dir(path).await?;
                Ok(().into())
            }
            Request::RepositorySetPexEnabled {
                repository,
                enabled,
            } => {
                self.state
                    .set_repository_pex_enabled(repository, enabled)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetQuota { repository, quota } => {
                self.state.set_repository_quota(repository, quota).await?;
                Ok(().into())
            }
            Request::RepositorySetRepositoryExpiration { repository, value } => {
                self.state
                    .set_repository_expiration(repository, value)
                    .await?;
                Ok(().into())
            }
            Request::RepositorySetStoreDir(path) => {
                self.state.set_store_dir(path).await?;
                Ok(().into())
            }
            Request::RepositorySetSyncEnabled {
                repository,
                enabled,
            } => {
                self.state
                    .set_repository_sync_enabled(repository, enabled)
                    .await?;
                Ok(().into())
            }
            Request::RepositoryShare {
                repository,
                secret,
                mode,
            } => Ok(self
                .state
                .share_repository(repository, secret, mode)
                .await?
                .into()),
            Request::RepositorySubscribe(repository) => {
                let rx = self.state.subscribe_to_repository(repository)?;
                self.subscriptions.insert((conn_id, message.id), rx.into());
                Ok(().into())
            }
            Request::RepositoryUnmount(repository) => {
                self.state.unmount_repository(repository).await?;
                Ok(().into())
            }
            Request::RepositorySyncProgress(repository) => Ok(self
                .state
                .repository_sync_progress(repository)
                .await?
                .into()),
            Request::ShareTokenMode(token) => Ok(token.access_mode().into()),
            Request::ShareTokenNormalize(token) => Ok(token.into()),
            Request::StateMonitorGet(path) => Ok(self.state.state_monitor(path)?.into()),
            Request::StateMonitorSubscribe(path) => {
                let rx = self.state.subscribe_to_state_monitor(path)?;
                self.subscriptions.insert((conn_id, message.id), rx.into());
                Ok(().into())
            }
            Request::Unsubscribe(id) => {
                self.subscriptions.remove(&(conn_id, id));
                Ok(().into())
            }
        }
    }

    async fn send_message(&mut self, conn_id: ConnectionId, message: Message<ResponseResult>) {
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

        // Remove subscriptions
        let sub_ids: Vec<_> = self
            .subscriptions
            .keys()
            .filter(|(sub_conn_id, _)| *sub_conn_id == conn_id)
            .copied()
            .collect();
        for sub_id in sub_ids {
            self.subscriptions.remove(&sub_id);
        }
    }
}

type ConnectionId = usize;
type SubscriptionId = (ConnectionId, MessageId);

async fn maybe_accept(
    server: Option<&mut RemoteServer>,
) -> io::Result<(RemoteServerReader, RemoteServerWriter)> {
    if let Some(server) = server {
        server.accept().await
    } else {
        future::pending().await
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn already_running() {
        let temp_dir = TempDir::new().unwrap();

        let socket_path = temp_dir.path().join("sock");
        let mut service0 = Service::init(socket_path.clone(), temp_dir.path().join("config"))
            .await
            .unwrap();

        assert_matches!(
            Service::init(socket_path, temp_dir.path().join("config"),)
                .await
                .map(|_| ()),
            Err(Error::ServiceAlreadyRunning)
        );

        service0.close().await;
    }
}

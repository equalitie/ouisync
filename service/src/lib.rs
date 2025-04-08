pub mod ffi;
pub mod logger;
pub mod protocol;
pub mod transport;

mod config_keys;
mod config_store;
mod device_id;
mod dht_contacts;
mod error;
mod file;
mod metrics;
mod network;
mod repository;
mod state;
mod subscription;
mod tls;
mod utils;

#[cfg(test)]
mod test_utils;

pub use error::Error;

use config_store::{ConfigError, ConfigKey, ConfigStore};
use futures_util::{stream::FuturesUnordered, SinkExt};
use ouisync_macros::api;
use protocol::{
    DecodeError, Message, MessageId, NetworkDefaults, ProtocolError, RepositoryHandle, Request,
    Response, ResponseResult,
};
use slab::Slab;
use state::State;
use state_monitor::MonitorId;
use std::{
    convert::Infallible,
    future, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use subscription::SubscriptionStream;
use tokio::{
    select,
    time::{self, MissedTickBehavior},
};
use tokio_stream::{StreamExt, StreamMap, StreamNotifyClose};
use transport::{
    local::{AuthKey, LocalServer},
    remote::{AcceptedRemoteConnection, RemoteServer},
    AcceptedConnection, ClientError, ReadError, ServerReader, ServerWriter,
};

const REPOSITORY_EXPIRATION_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);

// Don't use comments here so the file can be parsed as json.
const LOCAL_CONTROL_PORT_KEY: ConfigKey<u16> = ConfigKey::new("local_control_port", "");
const LOCAL_CONTROL_AUTH_KEY_KEY: ConfigKey<AuthKey> = ConfigKey::new("local_control_auth_key", "");

const REMOTE_CONTROL_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("remote_control", "Remote control endpoint address");

pub struct Service {
    state: State,
    local_server: LocalServer,
    remote_server: Option<RemoteServer>,
    readers: StreamMap<ConnectionId, StreamNotifyClose<ServerReader>>,
    writers: Slab<ServerWriter>,
    subscriptions: StreamMap<SubscriptionId, SubscriptionStream>,
}

impl Service {
    pub async fn init(config_dir: PathBuf) -> Result<Self, Error> {
        let state = State::init(config_dir).await?;

        let local_port_entry = state.config.entry(LOCAL_CONTROL_PORT_KEY);
        let local_port = match local_port_entry.get().await {
            Ok(port) => port,
            Err(ConfigError::NotFound) => 0,
            Err(error) => return Err(error.into()),
        };

        let local_auth_key = fetch_local_control_auth_key(&state.config).await?;

        let local_server = LocalServer::bind(local_port, local_auth_key)
            .await
            .map_err(|error| match error.kind() {
                io::ErrorKind::AddrInUse => Error::ServiceAlreadyRunning,
                _ => Error::Bind(error),
            })?;

        if local_port == 0 {
            local_port_entry.set(&local_server.port()).await?;
        }

        let remote_server = match state.config.entry(REMOTE_CONTROL_KEY).get().await {
            Ok(addr) => Some(
                RemoteServer::bind(addr, state.tls_config.server().await?)
                    .await
                    .map_err(Error::Bind)?,
            ),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        Ok(Self {
            state,
            local_server,
            remote_server,
            readers: StreamMap::new(),
            writers: Slab::new(),
            subscriptions: StreamMap::new(),
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

        let mut accepted_conns = FuturesUnordered::new();

        loop {
            select! {
                result = self.local_server.accept() => {
                    let conn = result.map_err(Error::Accept)?;
                    accepted_conns.push(AcceptedConnection::Local(conn).finalize());
                }
                result = maybe_accept(self.remote_server.as_ref()) => {
                    let conn = result.map_err(Error::Accept)?;
                    accepted_conns.push(AcceptedConnection::Remote(conn).finalize());
                }
                Some(result) = accepted_conns.next() => {
                    if let Some((reader, writer)) = result {
                       self.insert_connection(reader, writer);
                    }
                }
                Some((conn_id, message)) = self.readers.next() => {
                    if let Some(message) = message {
                        self.handle_message(conn_id, message).await
                    } else {
                        self.remove_connection(conn_id)
                    }
                }
                Some(((conn_id, message_id), response)) = self.subscriptions.next() => {
                    tracing::trace!(id = ?message_id, payload = ?response, "sending notification");

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
        self.readers.clear();

        for mut writer in self.writers.drain() {
            if let Err(error) = writer.close().await {
                tracing::warn!(?error, "failed to close local connection");
            }
        }

        self.state.close().await;
    }

    pub fn local_port(&self) -> u16 {
        self.local_server.port()
    }

    pub fn local_auth_key(&self) -> &AuthKey {
        self.local_server.auth_key()
    }

    pub fn store_dir(&self) -> Option<&Path> {
        self.state.store_dir()
    }

    pub async fn set_store_dir(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        self.state.session_set_store_dir(path.into()).await
    }

    /// Initialize network according to the stored config.
    pub async fn init_network(&mut self) {
        self.state
            .session_init_network(NetworkDefaults {
                bind: vec![],
                port_forwarding_enabled: false,
                local_discovery_enabled: false,
            })
            .await
    }

    /// Enable or disable syncing for all currently open repos.
    pub async fn set_sync_enabled_all(&mut self, enabled: bool) -> Result<(), Error> {
        self.state.set_all_repositories_sync_enabled(enabled).await
    }

    pub fn enable_panic_monitor(&self) {
        self.state.enable_panic_monitor();
    }

    #[api]
    pub(crate) async fn remote_control_bind(
        &mut self,
        addr: Option<SocketAddr>,
    ) -> Result<u16, Error> {
        if let Some(addr) = addr {
            let config = self.state.tls_config.server().await?;
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
                let span = tracing::trace_span!("request", message = ?message.payload);
                let id = message.id;
                let start = Instant::now();

                let result = self.dispatch(conn_id, message).await;

                span.in_scope(|| tracing::trace!(?result, elapsed = ?start.elapsed()));

                let message = Message {
                    id,
                    payload: result.into(),
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

    async fn send_message(&mut self, conn_id: ConnectionId, message: Message<ResponseResult>) {
        let Some(writer) = self.writers.get_mut(conn_id) else {
            tracing::error!("connection not found");
            return;
        };

        match writer.send(message).await {
            Ok(()) => (),
            Err(error) => {
                tracing::error!(?error, "failed to send message");
                self.remove_connection(conn_id);
            }
        }
    }

    #[api]
    fn remote_control_get_listener_addr(&self) -> Option<SocketAddr> {
        self.remote_server
            .as_ref()
            .map(|server| server.local_addr())
    }

    #[api]
    fn repository_subscribe(
        &mut self,
        conn_id: ConnectionId,
        message_id: MessageId,
        repo: RepositoryHandle,
    ) -> Result<(), Error> {
        let rx = self.state.repository_subscribe(repo)?;
        self.subscriptions.insert((conn_id, message_id), rx.into());
        Ok(())
    }

    #[api]
    fn session_subscribe_to_network(&mut self, conn_id: ConnectionId, message_id: MessageId) {
        let rx = self.state.network.subscribe();
        self.subscriptions.insert((conn_id, message_id), rx.into());
    }

    #[api]
    fn session_subscribe_to_state_monitor(
        &mut self,
        conn_id: ConnectionId,
        message_id: MessageId,
        path: Vec<MonitorId>,
    ) -> Result<(), Error> {
        let rx = self.state.state_monitor_subscribe(path)?;
        self.subscriptions.insert((conn_id, message_id), rx.into());
        Ok(())
    }

    /// Cancel a subscription identified by the given message id. The message id should be the same
    /// that was used for sending the corresponding subscribe request.
    #[api]
    fn session_unsubscribe(&mut self, conn_id: ConnectionId, _: MessageId, id: MessageId) {
        self.subscriptions.remove(&(conn_id, id));
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

// The `Service::dispatch` method is auto-generated with `build.rs` from the `#[api]` annotated
// methods in `impl State`.
include!(concat!(env!("OUT_DIR"), "/service.rs"));

/// Returns the loopback TCP port and authentication key for establishing local connection to the
/// service.
pub async fn local_control_endpoint(config_path: &Path) -> Result<(u16, AuthKey), ClientError> {
    let store = ConfigStore::new(config_path);

    let port = store
        .entry(LOCAL_CONTROL_PORT_KEY)
        .get()
        .await
        .map_err(|_| ClientError::InvalidEndpoint)?;

    let auth_key = store
        .entry(LOCAL_CONTROL_AUTH_KEY_KEY)
        .get()
        .await
        .map_err(|_| ClientError::InvalidEndpoint)?;

    Ok((port, auth_key))
}

type ConnectionId = usize;
type SubscriptionId = (ConnectionId, MessageId);

async fn maybe_accept(server: Option<&RemoteServer>) -> io::Result<AcceptedRemoteConnection> {
    if let Some(server) = server {
        server.accept().await
    } else {
        future::pending().await
    }
}

async fn fetch_local_control_auth_key(config: &ConfigStore) -> Result<AuthKey, ConfigError> {
    let entry = config.entry(LOCAL_CONTROL_AUTH_KEY_KEY);

    match entry.get().await {
        Ok(key) => Ok(key),
        Err(ConfigError::NotFound) => {
            let key = AuthKey::random();
            entry.set(&key).await?;
            Ok(key)
        }
        Err(error) => Err(error),
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

        let mut service0 = Service::init(temp_dir.path().join("config")).await.unwrap();

        assert_matches!(
            Service::init(temp_dir.path().join("config"))
                .await
                .map(|_| ()),
            Err(Error::ServiceAlreadyRunning)
        );

        service0.close().await;
    }
}

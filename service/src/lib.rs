pub mod ffi;
pub mod logger;
pub mod protocol;
pub mod transport;

mod config_keys;
mod config_store;
mod connection;
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
use connection::Connection;
use futures_util::{stream::FuturesUnordered, StreamExt};
use protocol::NetworkDefaults;
use state::State;
use std::{
    convert::Infallible,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    select,
    sync::watch,
    time::{self, MissedTickBehavior},
};
use transport::{
    local::{AuthKey, LocalServer},
    AcceptedConnection, ClientError,
};

const REPOSITORY_EXPIRATION_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);

// Don't use comments here so the file can be parsed as json.
const LOCAL_CONTROL_PORT_KEY: ConfigKey<u16> = ConfigKey::new("local_control_port", "");
const LOCAL_CONTROL_AUTH_KEY_KEY: ConfigKey<AuthKey> = ConfigKey::new("local_control_auth_key", "");

pub struct Service {
    state: Arc<State>,
    local_server: LocalServer,
    connections: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send>>>,
    closed: watch::Sender<bool>,
}

impl Service {
    pub async fn init(config_dir: PathBuf) -> Result<Self, Error> {
        let config = ConfigStore::new(config_dir);

        let local_port_entry = config.entry(LOCAL_CONTROL_PORT_KEY);
        let local_port = match local_port_entry.get().await {
            Ok(port) => port,
            Err(ConfigError::NotFound) => 0,
            Err(error) => return Err(error.into()),
        };

        let local_auth_key = fetch_local_control_auth_key(&config).await?;

        let local_server = LocalServer::bind(local_port, local_auth_key)
            .await
            .map_err(|error| match error.kind() {
                io::ErrorKind::AddrInUse => Error::ServiceAlreadyRunning,
                _ => Error::Bind(error),
            })?;

        if local_port == 0 {
            local_port_entry.set(&local_server.port()).await?;
        }

        let state = State::init(config).await?;
        let state = Arc::new(state);

        Ok(Self {
            state,
            local_server,
            connections: FuturesUnordered::new(),
            closed: watch::Sender::new(false),
        })
    }

    /// Runs the service. The future returned from this function never completes (unless it errors)
    /// but it's safe to cancel and optionally restart any number of times.
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
                    let conn = result.map_err(Error::Accept)?;
                    self.start_connection(AcceptedConnection::Local(conn));
                }
                result = self.state.remote_accept() => {
                    let conn = result.map_err(Error::Accept)?;
                    self.start_connection(AcceptedConnection::Remote(conn));
                }
                Some(_) = self.connections.next() => (),
                _ = repo_expiration_interval.tick() => {
                    self.state.delete_expired_repositories().await;
                }
            }
        }
    }

    pub async fn close(&mut self) {
        // Signal connections to close themselves
        self.closed.send(true).ok();

        // Wait for each connection to shutdown
        let close_connections = async { while self.connections.next().await.is_some() {} };

        select! {
            _ = self.state.close() => (),
            _ = close_connections => (),
        }
    }

    pub fn local_port(&self) -> u16 {
        self.local_server.port()
    }

    pub fn local_auth_key(&self) -> &AuthKey {
        self.local_server.auth_key()
    }

    pub fn store_dir(&self) -> Option<PathBuf> {
        self.state.store_dir()
    }

    pub async fn set_store_dir(&mut self, path: impl Into<PathBuf>) -> Result<(), Error> {
        self.state.session_set_store_dir(path.into()).await
    }

    /// Initialize network according to the stored config.
    pub async fn init_network(&self) {
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

    #[cfg(test)]
    pub(crate) fn state(&self) -> &State {
        &self.state
    }

    fn start_connection(&self, accepted: AcceptedConnection) {
        let state = self.state.clone();
        let mut closed_rx = self.closed.subscribe();

        let task = async move {
            let Some((reader, writer)) = accepted.finalize().await else {
                return;
            };

            let mut conn = Connection::new(reader, writer);

            let closed = select! {
                _ = conn.run(&state) => false,
                _ = closed_rx.wait_for(|closed| *closed) => true,
            };

            if closed {
                conn.close().await;
            }
        };

        self.connections.push(Box::pin(task));
    }
}

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

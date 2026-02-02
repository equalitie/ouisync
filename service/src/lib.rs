pub mod ffi;
pub mod logger;
pub mod protocol;
pub mod transport;

mod config_keys;
mod config_migration;
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

#[cfg(test)]
mod test_utils;

pub use error::Error;

use config_store::{ConfigError, ConfigKey, ConfigStore};
use connection::Connection;
use futures_util::{StreamExt, future, stream::FuturesUnordered};
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
    AcceptedConnection, ClientError,
    local::{AuthKey, LocalEndpoint, LocalServer},
};

const REPOSITORY_EXPIRATION_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);

// Don't use comments here so the file can be parsed as json.
const LOCAL_ENDPOINT_KEY: ConfigKey<LocalEndpoint> = ConfigKey::new("local_endpoint", "").private();

pub struct Service {
    state: Arc<State>,
    local_server: LocalServer,
    connections: FuturesUnordered<Pin<Box<dyn Future<Output = ()> + Send>>>,
    closed: watch::Sender<bool>,
}

impl Service {
    pub async fn init(config_dir: PathBuf) -> Result<Self, Error> {
        let config = ConfigStore::new(config_dir);

        config_migration::run(&config).await;

        let local_endpoint_entry = config.entry(LOCAL_ENDPOINT_KEY);
        let local_endpoint = match local_endpoint_entry.get().await {
            Ok(value) => value,
            Err(ConfigError::NotFound) => LocalEndpoint {
                port: 0,
                auth_key: AuthKey::random(),
            },
            Err(error) => return Err(error.into()),
        };

        let local_server = LocalServer::bind(local_endpoint)
            .await
            .map_err(|error| match error.kind() {
                io::ErrorKind::AddrInUse => Error::ServiceAlreadyRunning,
                _ => Error::Bind(error),
            })?;

        local_endpoint_entry.set(local_server.endpoint()).await?;

        tracing::debug!(
            port = local_server.endpoint().port,
            "local server listening"
        );

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

        // Wait for each connection to shutdown and close the state.
        future::join(
            self.state.close(),
            self.connections.by_ref().collect::<()>(),
        )
        .await;
    }

    pub fn local_endpoint(&self) -> &LocalEndpoint {
        self.local_server.endpoint()
    }

    pub fn store_dirs(&self) -> Vec<PathBuf> {
        self.state.store_dirs()
    }

    pub async fn set_store_dirs(&mut self, paths: Vec<PathBuf>) -> Result<(), Error> {
        self.state.session_set_store_dirs(paths).await
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
pub async fn local_endpoint(config_path: &Path) -> Result<LocalEndpoint, ClientError> {
    ConfigStore::new(config_path)
        .entry(LOCAL_ENDPOINT_KEY)
        .get()
        .await
        .map_err(ClientError::InvalidEndpoint)
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

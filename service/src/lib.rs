pub mod any_entry;
pub mod ffi;
pub mod logger;
pub mod protocol;
pub mod transport;

mod async_counter;
mod config_keys;
mod config_migration;
mod config_store;
mod connection;
mod coop_rw_lock;
mod device_id;
mod dht;
mod error;
mod file;
mod futures_map;
mod metrics;
mod network;
mod repository;
mod socket;
mod state;
mod stream;
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
    net::Ipv4Addr,
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
    // `None` only transiently, while rebinding after the old listener died - see
    // `rebind_local_server`.
    local_server: Option<LocalServer>,
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
                addr: Ipv4Addr::LOCALHOST,
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
            local_server: Some(local_server),
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
                result = self.local_server.as_ref().expect(
                    "local_server is only ever None transiently inside rebind_local_server, \
                     which this branch's body awaits to completion before looping back here"
                ).accept() => {
                    match result {
                        Ok(conn) => self.start_connection(AcceptedConnection::Local(conn)),
                        // The local listener's socket can be invalidated by the OS independently
                        // of anything we did (e.g. iOS tearing down a backgrounded app's sockets),
                        // and nothing else will ever recreate it - propagating this with `?` would
                        // permanently kill the entire service (all local AND remote connections,
                        // repository expiration, everything), not just local connections, since
                        // this whole function "never completes (unless it errors)". Rebind instead.
                        Err(error) => {
                            tracing::warn!(?error, "local server accept failed, rebinding");
                            self.rebind_local_server().await;
                        }
                    }
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

    /// Rebinds the local server listener to the same endpoint it was previously bound to.
    /// Retries with capped exponential backoff instead of giving up, since giving up here would
    /// mean no local client (i.e. the app's own UI) could ever talk to this service again until
    /// restarted.
    async fn rebind_local_server(&mut self) {
        let endpoint = *self.local_server.as_ref().unwrap().endpoint();

        // Drop the old listener *before* trying to bind the new one: even though its socket may
        // already be invalid at the OS level (that's exactly why we're here), our process still
        // holds a reference to it until it's dropped, which is enough to make the OS refuse to
        // bind a new socket to the same port (`AddrInUse`).
        self.local_server = None;

        let mut delay = Duration::from_millis(50);
        const MAX_DELAY: Duration = Duration::from_secs(5);

        loop {
            match LocalServer::bind(endpoint).await {
                Ok(server) => {
                    tracing::info!(port = server.endpoint().port, "local server rebound");
                    self.local_server = Some(server);
                    return;
                }
                Err(error) => {
                    tracing::warn!(?error, ?delay, "local server rebind failed, retrying");
                    time::sleep(delay).await;
                    delay = (delay * 2).min(MAX_DELAY);
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
        self.local_server.as_ref().unwrap().endpoint()
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
    use std::{net::Ipv4Addr, time::SystemTime};

    use assert_matches::assert_matches;
    use ouisync::WriteSecrets;
    use rcgen::{Certificate, CertificateParams, KeyPair};
    use tempfile::TempDir;
    use tokio::fs;
    use tokio_rustls::rustls::{self, ClientConfig};

    use crate::{
        test_utils::{ServiceRunner, init_log},
        tls,
        transport::remote::RemoteClient,
    };

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

    // Regression test for the local listener's socket being invalidated independently of
    // anything the service did (e.g. iOS tearing down a backgrounded app's sockets): the service
    // must rebind and keep accepting new local connections on the same port, instead of its
    // whole run loop dying because one `accept()` call failed.
    #[tokio::test]
    async fn rebind_local_server() {
        let temp_dir = TempDir::new().unwrap();
        let mut service = Service::init(temp_dir.path().join("config")).await.unwrap();
        let port = service.local_endpoint().port;

        // Exercise the same rebind path `run()` takes on an accept error, without needing to
        // simulate the OS invalidating the fd (which is OS/platform specific) - this still
        // covers the tricky part: binding to the *same* port while the old (still live, not yet
        // dropped) listener technically still holds it.
        time::timeout(Duration::from_secs(5), service.rebind_local_server())
            .await
            .expect("rebind_local_server timed out");

        assert_eq!(service.local_endpoint().port, port);

        let runner = ServiceRunner::start(service);

        // A fresh connection to the same port must still be accepted after rebinding.
        tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("failed to connect after rebind");

        let mut service = runner.stop().await;
        service.close().await;
    }

    #[tokio::test]
    async fn tls_cert_renewal() {
        async fn gen_cert(expires_in: Duration) -> (Certificate, KeyPair) {
            let signing_key = KeyPair::generate().unwrap();

            let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
            params.not_before = SystemTime::UNIX_EPOCH.into();
            params.not_after = (tls::test_utils::now() + expires_in).into();

            let cert = params.self_signed(&signing_key).unwrap();

            (cert, signing_key)
        }

        fn create_client_config(trusted_cert: &Certificate) -> Arc<ClientConfig> {
            let mut root_cert_store = rustls::RootCertStore::empty();
            root_cert_store.add(trusted_cert.der().clone()).unwrap();

            let mut config = rustls::ClientConfig::builder()
                .with_root_certificates(root_cert_store)
                .with_no_client_auth();
            config.time_provider = Arc::new(tls::test_utils::TokioTimeProvider);

            Arc::new(config)
        }

        async fn save_cert(config_dir: &Path, cert: &Certificate, signing_key: &KeyPair) {
            fs::write(config_dir.join("cert.pem"), &cert.pem())
                .await
                .unwrap();
            fs::write(config_dir.join("key.pem"), signing_key.serialize_pem())
                .await
                .unwrap();
        }

        init_log();

        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().join("config");

        let service = Service::init(config_dir.clone()).await.unwrap();

        let (cert_old, signing_key_old) = gen_cert(Duration::from_hours(24)).await;
        let (cert_new, signing_key_new) = gen_cert(Duration::from_hours(48)).await;

        // Setup the server to use the first cert
        save_cert(&config_dir, &cert_old, &signing_key_old).await;

        let port = service
            .state()
            .session_bind_remote_control(Some((Ipv4Addr::LOCALHOST, 0).into()))
            .await
            .unwrap();
        let host = format!("localhost:{port}");

        let service_runner = ServiceRunner::start(service);

        let repo_secrets = WriteSecrets::random();

        // This request succeeds because the trusted cert is still valid
        let mut client = RemoteClient::connect(&host, create_client_config(&cert_old))
            .await
            .unwrap();
        assert_matches!(client.mirror_exists(&repo_secrets.id).await, Ok(false));
        client.close().await;

        // Advance the time to expire the first cert
        //
        // NOTE: Using paused time so this test doesn't actually take 25 hours. Not pausing time for
        // the whole test because that was triggering very large time advances which caused the
        // cert to expire prematurely.
        time::pause();
        time::sleep(Duration::from_hours(25)).await;
        time::resume();

        // This request fails because the trusted cert expired
        assert_matches!(
            RemoteClient::connect(&host, create_client_config(&cert_old)).await,
            Err(ClientError::Connect(_))
        );

        // This request fails too because the server is still using the old cert
        assert_matches!(
            RemoteClient::connect(&host, create_client_config(&cert_new)).await,
            Err(ClientError::Connect(_))
        );

        // Renew the cert
        save_cert(&config_dir, &cert_new, &signing_key_new).await;

        // The cert reload happens in the background and there is currently no way to tell when it
        // completes. Let's try a couple of times before giving up.
        let mut client = retry(|| RemoteClient::connect(&host, create_client_config(&cert_new)))
            .await
            .unwrap();

        assert_matches!(client.mirror_exists(&repo_secrets.id).await, Ok(false));
        client.close().await;

        // Cleanup
        service_runner.stop().await.close().await;
    }

    async fn retry<Fun, Fut, T, E>(mut f: Fun) -> Result<T, E>
    where
        Fun: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut last_error = None;

        match time::timeout(Duration::from_secs(5), async {
            loop {
                match f().await {
                    Ok(value) => break value,
                    Err(error) => {
                        last_error = Some(error);
                        continue;
                    }
                }
            }
        })
        .await
        {
            Ok(result) => Ok(result),
            Err(_) => {
                if let Some(error) = last_error {
                    Err(error)
                } else {
                    panic!("timeout")
                }
            }
        }
    }
}

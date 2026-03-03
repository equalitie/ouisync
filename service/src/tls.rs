use std::{
    fmt, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{
    fs,
    runtime::Handle,
    sync::{OnceCell, watch},
};
use tokio_rustls::rustls::{
    self,
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivateKeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use tracing::Instrument;

use crate::Error;

const CERT_FILE_NAME: &str = "cert.pem";
const KEY_FILE_NAME: &str = "key.pem";

pub(crate) struct TlsConfig {
    dir: PathBuf,
    server: OnceCell<Arc<rustls::ServerConfig>>,
    client: OnceCell<Arc<rustls::ClientConfig>>,
}

impl TlsConfig {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            server: OnceCell::new(),
            client: OnceCell::new(),
        }
    }

    pub async fn server(&self) -> Result<Arc<rustls::ServerConfig>, Error> {
        self.server
            .get_or_try_init(|| make_server_config(&self.dir))
            .await
            .cloned()
    }

    pub async fn client(&self) -> Result<Arc<rustls::ClientConfig>, Error> {
        self.client
            .get_or_try_init(|| make_client_config(&self.dir))
            .await
            .cloned()
    }
}

pub(crate) async fn make_server_config(
    config_dir: &Path,
) -> Result<Arc<rustls::ServerConfig>, Error> {
    let builder = rustls::ServerConfig::builder().with_no_client_auth();

    let resolver = ServerCertResolver::new(config_dir, builder.crypto_provider().clone()).await?;
    let resolver = Arc::new(resolver);

    let config = builder.with_cert_resolver(resolver);

    #[cfg(test)]
    let config = {
        let mut config = config;
        config.time_provider = Arc::new(test_utils::TokioTimeProvider);
        config
    };

    Ok(Arc::new(config))
}

pub(crate) async fn make_client_config(
    config_dir: &Path,
) -> Result<Arc<rustls::ClientConfig>, Error> {
    // Load custom root certificates (if any)
    let additional_root_certs = load_certificates_from_dir(&config_dir.join("root_certs"))
        .await
        .inspect_err(|error| tracing::error!(?error, "failed to load TLS root certificates"))
        .map_err(Error::TlsCertificatesInvalid)?;

    let mut root_cert_store = rustls::RootCertStore::empty();

    // Add default root certificates
    root_cert_store.extend(
        webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .map(|ta| ta.to_owned()),
    );

    // Add custom root certificates (if any)
    for cert in additional_root_certs {
        root_cert_store
            .add(cert.clone())
            .inspect_err(|error| tracing::error!(?error, "failed to add TLS root certificate"))
            .map_err(Error::TlsConfig)?;
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();

    #[cfg(test)]
    let config = {
        let mut config = config;
        config.time_provider = Arc::new(test_utils::TokioTimeProvider);
        config
    };

    Ok(Arc::new(config))
}

/// Certificate resolver that automatically reloads the certificate when it changes on the disk.
struct ServerCertResolver {
    current_rx: watch::Receiver<Arc<CertifiedKey>>,
    _watcher: RecommendedWatcher,
}

impl ServerCertResolver {
    async fn new(dir: &Path, provider: Arc<CryptoProvider>) -> Result<Self, Error> {
        let current = load_certified_key(dir, &provider).await?;
        let current = Arc::new(current);

        let current_tx = watch::Sender::new(current);
        let current_rx = current_tx.subscribe();

        // Use the `notify` crate to watch for changes of the cert or key file.
        let handle = Handle::current().clone();
        let mut watcher = notify::recommended_watcher(move |event: Result<Event, _>| {
            let Ok(event) = event else {
                return;
            };

            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                return;
            }

            let Some(dir) = event
                .paths
                .into_iter()
                .find(|path| path.ends_with(CERT_FILE_NAME) || path.ends_with(KEY_FILE_NAME))
                .map(|mut path| {
                    path.pop();
                    path
                })
            else {
                return;
            };

            let provider = provider.clone();
            let current_tx = current_tx.clone();

            handle.spawn(async move {
                if let Ok(key) = load_certified_key(&dir, &provider)
                    .instrument(tracing::info_span!("TLS certificate reload"))
                    .await
                {
                    tracing::debug!("TLS certificate reloaded");
                    current_tx.send(Arc::new(key)).ok();
                }
            });
        })?;

        watcher.watch(&dir.join(CERT_FILE_NAME), RecursiveMode::NonRecursive)?;
        watcher.watch(&dir.join(KEY_FILE_NAME), RecursiveMode::NonRecursive)?;

        Ok(Self {
            current_rx,
            _watcher: watcher,
        })
    }
}

impl ResolvesServerCert for ServerCertResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current_rx.borrow().clone())
    }
}

impl fmt::Debug for ServerCertResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerCertResolver").finish_non_exhaustive()
    }
}

async fn load_certified_key(dir: &Path, provider: &CryptoProvider) -> Result<CertifiedKey, Error> {
    let cert_path = dir.join(CERT_FILE_NAME);
    let key_path = dir.join(KEY_FILE_NAME);

    let certs = load_certificates_from_file(&cert_path)
        .await
        .map_err(|error| {
            tracing::error!(
                ?error,
                "failed to load TLS certificate from {}",
                cert_path.display()
            );

            Error::TlsCertificatesNotFound
        })?;

    if certs.is_empty() {
        tracing::error!(
            "failed to load TLS certificate from {}: no certificates found",
            cert_path.display()
        );

        return Err(Error::TlsCertificatesNotFound);
    }

    let keys = load_keys_from_file(&key_path).await.map_err(|error| {
        tracing::error!(?error, "failed to load TLS key from {}", key_path.display(),);

        Error::TlsKeysNotFound
    })?;

    let key = keys.into_iter().next().ok_or_else(|| {
        tracing::error!(
            "failed to load TLS key from {}: no keys found",
            key_path.display()
        );

        Error::TlsKeysNotFound
    })?;

    CertifiedKey::from_der(certs, key, provider).map_err(|error| {
        tracing::error!(?error, "failed to create TLS certified key");
        Error::TlsConfig(error)
    })
}

/// Loads all certificates in the given directory (non-recursively).
async fn load_certificates_from_dir(dir: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut read_dir = match fs::read_dir(dir).await {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };

    let mut certs = Vec::new();

    while let Some(entry) = read_dir.next_entry().await? {
        if !entry.file_type().await?.is_file() {
            continue;
        }

        let path = entry.path();

        match path.extension().and_then(|e| e.to_str()) {
            Some("pem" | "crt") => (),
            Some(_) | None => continue,
        }

        certs.extend(load_certificates_from_file(entry.path()).await?);
    }

    Ok(certs)
}

/// Loads certificates from the given file.
async fn load_certificates_from_file(
    path: impl AsRef<Path>,
) -> io::Result<Vec<CertificateDer<'static>>> {
    load_pems(path.as_ref(), &["CERTIFICATE"])
        .await
        .map(|pems| pems.map(|content| content.into()).collect())
}

/// Loads private keys from the given file.
pub async fn load_keys_from_file(
    path: impl AsRef<Path>,
) -> io::Result<Vec<PrivateKeyDer<'static>>> {
    let pems = load_pems(
        path.as_ref(),
        &["PRIVATE KEY", "EC PRIVATE KEY", "RSA PRIVATE KEY"],
    )
    .await?;

    pems.map(|content| {
        PrivateKeyDer::try_from(content)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    })
    .collect()
}

async fn load_pems<'a>(
    path: &Path,
    tags: &'a [&str],
) -> io::Result<impl Iterator<Item = Vec<u8>> + 'a> {
    let content = fs::read(path).await?;

    let pems = pem::parse_many(content)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    Ok(pems
        .into_iter()
        .filter(move |pem| tags.contains(&pem.tag()))
        .map(|pem| pem.into_contents()))
}

#[cfg(test)]
pub(crate) mod test_utils {
    use std::{sync::LazyLock, time::SystemTime};

    use tokio::time::Instant;
    use tokio_rustls::rustls::{pki_types::UnixTime, time_provider::TimeProvider};

    // Fake unix epoch
    static UNIX_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

    /// rustls's `TimeProvider` impl based on tokio's simulated time.
    #[derive(Debug)]
    pub struct TokioTimeProvider;

    impl TimeProvider for TokioTimeProvider {
        fn current_time(&self) -> Option<UnixTime> {
            Some(UnixTime::since_unix_epoch(UNIX_EPOCH.elapsed()))
        }
    }

    /// Fake current time as `SystemTime`
    pub fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + UNIX_EPOCH.elapsed()
    }
}

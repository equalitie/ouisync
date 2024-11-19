use crate::{
    error::Error,
    protocol::Pattern,
    repository::{RepositoryHandle, RepositoryHolder, RepositorySet},
    utils,
};
use ouisync::{Network, SetLocalSecret, ShareToken};
use ouisync_bridge::{
    config::{ConfigKey, ConfigStore},
    network::{self, NetworkDefaults},
    transport::tls,
};
use state_monitor::StateMonitor;
use std::{
    borrow::Cow,
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{fs, sync::OnceCell};
use tokio_rustls::rustls;
use tokio_stream::StreamExt;

const STORE_DIR_KEY: ConfigKey<PathBuf> =
    ConfigKey::new("store_dir", "Repository storage directory");

const DEFAULT_REPOSITORY_EXPIRATION_KEY: ConfigKey<u64> = ConfigKey::new(
    "default_repository_expiration",
    "Default time in milliseconds after repository is deleted if all its blocks expired",
);

const REPOSITORY_FILE_EXTENSION: &str = "ouisyncdb";

pub(crate) struct State {
    pub config: ConfigStore,
    pub network: Network,
    store_dir: Option<PathBuf>,
    repos: RepositorySet,
    repos_monitor: StateMonitor,
    remote_server_config: OnceCell<Arc<rustls::ServerConfig>>,
    remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
}

impl State {
    pub async fn init(config_dir: &Path) -> Result<Self, Error> {
        let config = ConfigStore::new(config_dir);
        let monitor = StateMonitor::make_root();

        let network = Network::new(
            monitor.make_child("Network"),
            Some(config.dht_contacts_store()),
            None,
        );

        network::init(
            &network,
            &config,
            NetworkDefaults {
                port_forwarding_enabled: false,
                local_discovery_enabled: false,
            },
        )
        .await;

        let store_dir = config
            .entry(STORE_DIR_KEY)
            .get()
            .await
            .inspect_err(|error| tracing::warn!(?error, "failed to get store dir from config"))
            .ok();

        let repos_monitor = monitor.make_child("Repositories");

        let repos = if let Some(dir) = &store_dir {
            find_repositories(dir, &config, &network, &repos_monitor).await
        } else {
            RepositorySet::new()
        };

        // TODO:
        // const REPOSITORY_EXPIRATION_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
        // let state = State::init(&dirs)
        //     .await?
        //     .start_delete_expired_repositories(REPOSITORY_EXPIRATION_POLL_INTERVAL);

        Ok(Self {
            config,
            network,
            store_dir,
            repos_monitor,
            repos,
            remote_server_config: OnceCell::new(),
            remote_client_config: OnceCell::new(),
        })
    }

    pub fn store_dir(&self) -> Option<&Path> {
        self.store_dir.as_deref()
    }

    pub async fn set_store_dir(&mut self, dir: PathBuf) -> Result<(), Error> {
        self.config.entry(STORE_DIR_KEY).set(&dir).await?;
        self.store_dir = Some(dir.clone());
        Ok(())
    }

    pub fn find_repositories<'a>(
        &'a self,
        pattern: &'a Pattern,
    ) -> impl Iterator<Item = RepositoryHandle> + 'a {
        self.repos.find(pattern)
    }

    pub async fn create_repository(
        &mut self,
        name: String,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        share_token: Option<ShareToken>,
    ) -> Result<RepositoryHandle, Error> {
        if self
            .repos
            .find(&Pattern::Exact(name.clone()))
            .next()
            .is_some()
        {
            Err(Error::RepositoryExists)?;
        }

        let store_path = self.store_path(name.as_ref())?;

        let repository = ouisync_bridge::repository::create(
            store_path,
            read_secret,
            write_secret,
            share_token,
            &self.config,
            &self.repos_monitor,
        )
        .await?;

        let registration = self.network.register(repository.handle()).await;
        let mut holder = RepositoryHolder::new(name.clone(), repository);
        holder.enable_sync(registration);

        let value = self.default_repository_expiration().await?;
        holder.set_repository_expiration(value).await?;

        let handle = self
            .repos
            .try_insert(holder)
            .ok_or(Error::RepositoryExists)?;

        tracing::info!(name, "repository created");

        Ok(handle)
    }

    pub async fn delete_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let Some(mut holder) = self.repos.remove(handle) else {
            return Ok(());
        };

        holder.close().await?;

        let store_path = self.store_path(holder.name())?;

        ouisync::delete_repository(&store_path).await?;

        // Remove ancestors directories up to `store_dir` but only if they are empty.
        for path in store_path.ancestors().skip(1) {
            if Some(path) == self.store_dir.as_deref() {
                break;
            }

            // TODO: When `io::ErrorKind::DirectoryNotEmpty` is stabilized, we should break only on that
            // error and propagate the rest.
            if let Err(error) = fs::remove_dir(&path).await {
                tracing::error!(
                    repo = holder.name(),
                    path = %path.display(),
                    ?error,
                    "failed to remove repository store subdirectory"
                );
                break;
            }
        }

        tracing::info!(name = holder.name(), "repository deleted");

        Ok(())
    }

    pub async fn export_repository(
        &self,
        handle: RepositoryHandle,
        output: String,
    ) -> Result<String, Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;

        let output = PathBuf::from(output);
        let output = if output.extension().is_some() {
            output
        } else {
            output.with_extension(REPOSITORY_FILE_EXTENSION)
        };

        holder.repository().export(&output).await?;

        Ok(output.to_string_lossy().into_owned().into())
    }

    pub async fn set_default_repository_expiration(
        &self,
        value: Option<Duration>,
    ) -> Result<(), Error> {
        let entry = self.config.entry(DEFAULT_REPOSITORY_EXPIRATION_KEY);

        if let Some(value) = value {
            entry
                .set(&value.as_millis().try_into().unwrap_or(u64::MAX))
                .await?;
        } else {
            entry.remove().await?;
        }

        Ok(())
    }

    pub async fn default_repository_expiration(&self) -> Result<Option<Duration>, Error> {
        let entry = self.config.entry::<u64>(DEFAULT_REPOSITORY_EXPIRATION_KEY);

        match entry.get().await {
            Ok(millis) => Ok(Some(Duration::from_millis(millis))),
            Err(ouisync_bridge::config::ConfigError::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn remote_server_config(&self) -> Result<Arc<rustls::ServerConfig>, Error> {
        self.remote_server_config
            .get_or_try_init(|| make_server_config(self.config.dir()))
            .await
            .cloned()
    }

    pub async fn remote_client_config(&self) -> Result<Arc<rustls::ClientConfig>, Error> {
        self.remote_client_config
            .get_or_try_init(|| make_client_config(self.config.dir()))
            .await
            .cloned()
    }

    pub async fn close(&mut self) -> Result<(), Error> {
        todo!()
    }

    fn store_path(&self, repo_name: &str) -> Result<PathBuf, Error> {
        let store_dir = self
            .store_dir
            .as_deref()
            .ok_or(Error::StoreDirUnspecified)?;

        let suffix = Path::new(repo_name);
        let extension = if let Some(extension) = suffix.extension() {
            let mut extension = extension.to_owned();
            extension.push(".");
            extension.push(REPOSITORY_FILE_EXTENSION);

            Cow::Owned(extension)
        } else {
            Cow::Borrowed(REPOSITORY_FILE_EXTENSION.as_ref())
        };

        Ok(store_dir.join(suffix).with_extension(extension))
    }
}

// Find repositories that are marked to be opened on startup and open them.
pub(crate) async fn find_repositories(
    store_dir: &Path,
    config: &ConfigStore,
    network: &Network,
    monitor: &StateMonitor,
) -> RepositorySet {
    let mut repositories = RepositorySet::new();

    if !fs::try_exists(store_dir).await.unwrap_or(false) {
        tracing::error!("store dir doesn't exist");
        return repositories;
    }

    let mut walkdir = utils::walk_dir(store_dir);

    while let Some(entry) = walkdir.next().await {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::error!(?error, "failed to read directory entry");
                continue;
            }
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();

        if path.extension() != Some(OsStr::new(REPOSITORY_FILE_EXTENSION)) {
            continue;
        }

        let repo = match ouisync_bridge::repository::open(path.to_path_buf(), None, config, monitor)
            .await
        {
            Ok(repo) => repo,
            Err(error) => {
                tracing::error!(?error, ?path, "failed to open repository");
                continue;
            }
        };

        let name = path
            .strip_prefix(store_dir)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .into_owned();

        tracing::info!(%name, "Repository opened");

        let registration = network.register(repo.handle()).await;

        let mut holder = RepositoryHolder::new(name, repo);
        holder.enable_sync(registration);

        // TODO: mount
        //     holder.mount(&dirs.mount_dir).await.ok();

        let (_, old) = repositories.insert(holder);

        if let Some(old) = old {
            panic!(
                "multiple repositories with the same name found: \"{}\"",
                old.name()
            );
        }
    }

    repositories
}

async fn make_server_config(config_dir: &Path) -> Result<Arc<rustls::ServerConfig>, Error> {
    let cert_path = config_dir.join("cert.pem");
    let key_path = config_dir.join("key.pem");

    let certs = tls::load_certificates_from_file(&cert_path)
        .await
        .inspect_err(|error| {
            tracing::error!(
                "failed to load TLS certificate from {}: {}",
                cert_path.display(),
                error,
            )
        })?;

    if certs.is_empty() {
        tracing::error!(
            "failed to load TLS certificate from {}: no certificates found",
            cert_path.display()
        );

        return Err(Error::TlsCertificatesNotFound);
    }

    let keys = tls::load_keys_from_file(&key_path)
        .await
        .inspect_err(|error| {
            tracing::error!(
                "failed to load TLS key from {}: {}",
                key_path.display(),
                error
            )
        })?;

    let key = keys.into_iter().next().ok_or_else(|| {
        tracing::error!(
            "failed to load TLS key from {}: no keys found",
            cert_path.display()
        );

        Error::TlsKeysNotFound
    })?;

    Ok(ouisync_bridge::transport::make_server_config(certs, key)?)
}

async fn make_client_config(config_dir: &Path) -> Result<Arc<rustls::ClientConfig>, Error> {
    // Load custom root certificates (if any)
    let additional_root_certs =
        tls::load_certificates_from_dir(&config_dir.join("root_certs")).await?;
    Ok(ouisync_bridge::transport::make_client_config(
        &additional_root_certs,
    )?)
}

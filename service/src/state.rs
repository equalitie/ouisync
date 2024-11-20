use crate::{
    error::Error,
    protocol::ImportMode,
    repository::{FindError, RepositoryHandle, RepositoryHolder, RepositorySet},
    utils,
};
use ouisync::{AccessMode, LocalSecret, Network, SetLocalSecret, ShareToken};
use ouisync_bridge::{
    config::{ConfigKey, ConfigStore},
    network::{self, NetworkDefaults},
    transport::tls,
};
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use state_monitor::StateMonitor;
use std::{
    borrow::Cow,
    collections::BTreeMap,
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

const MOUNT_DIR_KEY: ConfigKey<PathBuf> = ConfigKey::new("mount_dir", "Repository mount directory");

const DEFAULT_REPOSITORY_EXPIRATION_KEY: ConfigKey<u64> = ConfigKey::new(
    "default_repository_expiration",
    "Default time in milliseconds after repository is deleted if all its blocks expired",
);

const REPOSITORY_FILE_EXTENSION: &str = "ouisyncdb";

pub(crate) struct State {
    pub config: ConfigStore,
    pub network: Network,
    store_dir: Option<PathBuf>,
    mount_dir: Option<PathBuf>,
    mounter: Option<MultiRepoVFS>,
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

        let mount_dir = config
            .entry(MOUNT_DIR_KEY)
            .get()
            .await
            .inspect_err(|error| tracing::warn!(?error, "failed to get mount dir from config"))
            .ok();

        let repos_monitor = monitor.make_child("Repositories");

        // TODO:
        // const REPOSITORY_EXPIRATION_POLL_INTERVAL: Duration = Duration::from_secs(60 * 60);
        // let state = State::init(&dirs)
        //     .await?
        //     .start_delete_expired_repositories(REPOSITORY_EXPIRATION_POLL_INTERVAL);

        let mut state = Self {
            config,
            network,
            store_dir,
            mount_dir,
            mounter: None,
            repos_monitor,
            repos: RepositorySet::new(),
            remote_server_config: OnceCell::new(),
            remote_client_config: OnceCell::new(),
        };

        state.load_repositories().await;

        Ok(state)
    }

    pub fn store_dir(&self) -> Option<&Path> {
        self.store_dir.as_deref()
    }

    pub async fn set_store_dir(&mut self, dir: PathBuf) -> Result<(), Error> {
        self.config.entry(STORE_DIR_KEY).set(&dir).await?;
        self.store_dir = Some(dir);
        Ok(())
    }

    pub fn mount_dir(&self) -> Option<&Path> {
        self.mount_dir.as_deref()
    }

    pub async fn set_mount_dir(&mut self, dir: PathBuf) -> Result<(), Error> {
        if Some(&dir) == self.mount_dir.as_ref() {
            return Ok(());
        }

        self.config.entry(MOUNT_DIR_KEY).set(&dir).await?;
        self.mount_dir = Some(dir);

        // TODO: remount all

        Ok(())
    }

    pub fn find_repository(&self, name: &str) -> Result<RepositoryHandle, FindError> {
        let (handle, _) = self.repos.find(name)?;
        Ok(handle)
    }

    pub fn list_repositories(&self) -> BTreeMap<String, RepositoryHandle> {
        self.repos
            .iter()
            .map(|(handle, holder)| (holder.name().to_owned(), handle))
            .collect()
    }

    pub async fn create_repository(
        &mut self,
        name: String,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        share_token: Option<ShareToken>,
    ) -> Result<RepositoryHandle, Error> {
        if self.repos.find(&name).is_ok() {
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
        output_path: PathBuf,
    ) -> Result<PathBuf, Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;

        let output_path = if output_path.extension().is_some() {
            output_path
        } else {
            output_path.with_extension(REPOSITORY_FILE_EXTENSION)
        };

        holder.repository().export(&output_path).await?;

        Ok(output_path)
    }

    pub async fn import_repository(
        &mut self,
        input_path: PathBuf,
        name: Option<String>,
        mode: ImportMode,
        force: bool,
    ) -> Result<RepositoryHandle, Error> {
        let name = if let Some(name) = name {
            name
        } else {
            input_path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let store_path = self.store_path(&name)?;

        if fs::try_exists(&store_path).await? {
            if force {
                if let Ok((handle, _)) = self.repos.find(&name) {
                    if let Some(mut holder) = self.repos.remove(handle) {
                        holder.close().await?;
                    }
                }
            } else {
                return Err(Error::RepositoryExists);
            }
        }

        match mode {
            ImportMode::Copy => {
                fs::copy(input_path, &store_path).await?;
            }
            ImportMode::Move => {
                fs::rename(input_path, &store_path).await?;
            }
            ImportMode::SoftLink => {
                #[cfg(unix)]
                fs::symlink(input_path, &store_path).await?;

                #[cfg(windows)]
                fs::symlink_file(input_path, &store_path).await?;

                #[cfg(not(any(unix, windows)))]
                return Err(Error::OperationNotSupported);
            }
            ImportMode::HardLink => {
                fs::hard_link(input_path, &store_path).await?;
            }
        }

        self.open_repository(&store_path).await
    }

    pub async fn share_repository(
        &self,
        handle: RepositoryHandle,
        secret: Option<LocalSecret>,
        mode: AccessMode,
    ) -> Result<String, Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;
        let token = ouisync_bridge::repository::create_share_token(
            holder.repository(),
            secret,
            mode,
            Some(holder.name().to_owned()),
        )
        .await?;

        Ok(token)
    }

    pub async fn mount_repository(&mut self, handle: RepositoryHandle) -> Result<PathBuf, Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;

        let mounter = match &self.mounter {
            Some(mounter) => mounter,
            None => {
                let mount_dir = self.mount_dir().ok_or(Error::MountDirUnspecified)?;
                let new_mounter = MultiRepoVFS::create(mount_dir).await?;
                self.mounter.insert(new_mounter)
            }
        };

        if let Some(mount_point) = mounter.mount_point(holder.name()) {
            // Already mounted
            Ok(mount_point)
        } else {
            Ok(mounter.insert(holder.name().to_owned(), holder.repository().clone())?)
        }
    }

    pub async fn unmount_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let Some(mounter) = self.mounter.as_ref() else {
            return Ok(());
        };

        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;
        mounter.remove(holder.name())?;

        Ok(())
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

    // Find all repositories in the store dir and open them.
    async fn load_repositories(&mut self) {
        let Some(store_dir) = self.store_dir.as_deref() else {
            tracing::error!("store dir isn't specified");
            return;
        };

        if !fs::try_exists(store_dir).await.unwrap_or(false) {
            tracing::error!("store dir doesn't exist");
            return;
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

            match self.open_repository(path).await {
                Ok(_) => (),
                Err(error) => {
                    tracing::error!(?error, ?path, "failed to open repository");
                    continue;
                }
            }
        }
    }

    async fn open_repository(&mut self, path: &Path) -> Result<RepositoryHandle, Error> {
        let repo = ouisync_bridge::repository::open(
            path.to_path_buf(),
            None,
            &self.config,
            &self.repos_monitor,
        )
        .await?;

        let name = self
            .store_dir
            .as_deref()
            .and_then(|store_dir| path.strip_prefix(store_dir).ok())
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .into_owned();

        tracing::info!(name, "repository opened");

        let registration = self.network.register(repo.handle()).await;

        let mut holder = RepositoryHolder::new(name, repo);
        holder.enable_sync(registration);

        // TODO: mount
        //     holder.mount(&dirs.mount_dir).await.ok();

        self.repos.try_insert(holder).ok_or(Error::RepositoryExists)
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

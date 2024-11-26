use crate::{
    error::Error,
    protocol::{ImportMode, QuotaInfo},
    repository::{FindError, RepositoryHandle, RepositoryHolder, RepositorySet},
    transport::remote::RemoteClient,
    utils, Defaults,
};
use ouisync::{
    AccessMode, Credentials, LocalSecret, Network, PeerAddr, SetLocalSecret, ShareToken,
    StorageSize,
};
use ouisync_bridge::{
    config::{ConfigError, ConfigKey, ConfigStore},
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

const AUTOMOUNT_KEY: &str = "automount";

const REPOSITORY_FILE_EXTENSION: &str = "ouisyncdb";

pub(crate) struct State {
    pub config: ConfigStore,
    pub network: Network,
    store_dir: PathBuf,
    mounter: MultiRepoVFS,
    repos: RepositorySet,
    repos_monitor: StateMonitor,
    remote_server_config: OnceCell<Arc<rustls::ServerConfig>>,
    remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
}

impl State {
    pub async fn init(config_dir: PathBuf, defaults: Defaults) -> Result<Self, Error> {
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
                port_forwarding_enabled: defaults.port_forwarding_enabled,
                local_discovery_enabled: defaults.local_discovery_enabled,
                bind: defaults.bind,
            },
        )
        .await;

        let store_dir = match config.entry(STORE_DIR_KEY).get().await {
            Ok(dir) => dir,
            Err(ConfigError::NotFound) => defaults.store_dir,
            Err(error) => return Err(error.into()),
        };

        let mount_dir = match config.entry(MOUNT_DIR_KEY).get().await {
            Ok(dir) => dir,
            Err(ConfigError::NotFound) => defaults.mount_dir,
            Err(error) => return Err(error.into()),
        };

        let mounter = MultiRepoVFS::create(mount_dir).await?;

        let repos_monitor = monitor.make_child("Repositories");

        let mut state = Self {
            config,
            network,
            store_dir,
            mounter,
            repos_monitor,
            repos: RepositorySet::new(),
            remote_server_config: OnceCell::new(),
            remote_client_config: OnceCell::new(),
        };

        state.load_repositories().await;

        Ok(state)
    }

    pub async fn bind_network(&self, addrs: Vec<PeerAddr>) {
        network::bind(&self.network, &self.config, &addrs).await
    }

    pub async fn add_user_provided_peers(&self, addrs: Vec<PeerAddr>) {
        ouisync_bridge::network::add_user_provided_peers(&self.network, &self.config, &addrs).await
    }

    pub async fn remove_user_provided_peers(&self, addrs: Vec<PeerAddr>) {
        ouisync_bridge::network::remove_user_provided_peers(&self.network, &self.config, &addrs)
            .await
    }

    pub async fn user_provided_peers(&self) -> Vec<PeerAddr> {
        ouisync_bridge::network::user_provided_peers(&self.config).await
    }

    pub async fn set_local_discovery_enabled(&self, enabled: bool) {
        ouisync_bridge::network::set_local_discovery_enabled(&self.network, &self.config, enabled)
            .await;
    }

    pub async fn set_port_forwarding_enabled(&self, enabled: bool) {
        ouisync_bridge::network::set_port_forwarding_enabled(&self.network, &self.config, enabled)
            .await
    }

    pub fn store_dir(&self) -> &Path {
        &self.store_dir
    }

    pub async fn set_store_dir(&mut self, dir: PathBuf) -> Result<(), Error> {
        if dir == self.store_dir {
            return Ok(());
        }

        self.config.entry(STORE_DIR_KEY).set(&dir).await?;
        self.store_dir = dir;

        // Close repos from the previous store dir and load repos from the new dir.
        self.close_repositories().await;
        self.load_repositories().await;

        Ok(())
    }

    pub fn mount_dir(&self) -> &Path {
        self.mounter.mount_root()
    }

    pub async fn set_mount_dir(&mut self, dir: PathBuf) -> Result<(), Error> {
        if dir == self.mounter.mount_root() {
            return Ok(());
        }

        self.config.entry(MOUNT_DIR_KEY).set(&dir).await?;
        self.mounter = MultiRepoVFS::create(dir).await?;

        // Remount all mounted repos
        for (_, holder) in self.repos.iter() {
            if holder
                .repository()
                .metadata()
                .get(AUTOMOUNT_KEY)
                .await?
                .unwrap_or(false)
            {
                self.mounter.remove(holder.name())?;
                self.mounter
                    .insert(holder.name().to_owned(), holder.repository().clone())?;
            }
        }

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
        enable_dht: bool,
        enable_pex: bool,
    ) -> Result<RepositoryHandle, Error> {
        if self.repos.find(&name).is_ok() {
            Err(Error::RepositoryExists)?;
        }

        let store_path = self.store_path(name.as_ref());

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
        registration.set_dht_enabled(enable_dht).await;
        registration.set_pex_enabled(enable_pex).await;

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

        let store_path = self.store_path(holder.name());

        ouisync::delete_repository(&store_path).await?;

        // Remove ancestors directories up to `store_dir` but only if they are empty.
        for path in store_path.ancestors().skip(1) {
            if *path == self.store_dir {
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
        let store_path = self.store_path(&name);

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
    ) -> Result<ShareToken, Error> {
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

    pub async fn reset_repository_access(
        &self,
        handle: RepositoryHandle,
        token: ShareToken,
    ) -> Result<(), Error> {
        let new_credentials = Credentials::with_random_writer_id(token.into_secrets());
        self.repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .repository()
            .set_credentials(new_credentials)
            .await?;

        Ok(())
    }

    pub async fn mount_repository(&mut self, handle: RepositoryHandle) -> Result<PathBuf, Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;

        holder
            .repository()
            .metadata()
            .set(AUTOMOUNT_KEY, true)
            .await?;

        if let Some(mount_point) = self.mounter.mount_point(holder.name()) {
            // Already mounted
            Ok(mount_point)
        } else {
            Ok(self
                .mounter
                .insert(holder.name().to_owned(), holder.repository().clone())?)
        }
    }

    pub async fn unmount_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;

        holder.repository().metadata().remove(AUTOMOUNT_KEY).await?;

        self.mounter.remove(holder.name())?;

        Ok(())
    }

    pub fn is_repository_dht_enabled(&self, handle: RepositoryHandle) -> Result<bool, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .registration()
            .ok_or(Error::RepositorySyncDisabled)?
            .is_dht_enabled())
    }

    pub async fn set_repository_dht_enabled(
        &self,
        handle: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .registration()
            .ok_or(Error::RepositorySyncDisabled)?
            .set_dht_enabled(enabled)
            .await;
        Ok(())
    }

    pub fn is_repository_pex_enabled(&self, handle: RepositoryHandle) -> Result<bool, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .registration()
            .ok_or(Error::RepositorySyncDisabled)?
            .is_pex_enabled())
    }

    pub async fn set_repository_pex_enabled(
        &self,
        handle: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .registration()
            .ok_or(Error::RepositorySyncDisabled)?
            .set_pex_enabled(enabled)
            .await;
        Ok(())
    }

    pub async fn set_pex_send_enabled(&self, enabled: bool) {
        ouisync_bridge::network::set_pex_send_enabled(&self.network, &self.config, enabled).await
    }

    pub async fn set_pex_recv_enabled(&self, enabled: bool) {
        ouisync_bridge::network::set_pex_recv_enabled(&self.network, &self.config, enabled).await
    }

    pub async fn create_repository_mirror(
        &self,
        handle: RepositoryHandle,
        host: String,
    ) -> Result<(), Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;
        let secrets = holder
            .repository()
            .secrets()
            .into_write_secrets()
            .ok_or(Error::PermissionDenied)?;

        let mut client = self.connect_remote_client(&host).await?;

        let result = client.create_mirror(&secrets).await;
        client.close().await;
        result?;

        Ok(())
    }

    pub async fn delete_repository_mirror(
        &self,
        handle: RepositoryHandle,
        host: String,
    ) -> Result<(), Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;
        let secrets = holder
            .repository()
            .secrets()
            .into_write_secrets()
            .ok_or(Error::PermissionDenied)?;

        let mut client = self.connect_remote_client(&host).await?;

        let result = client.delete_mirror(&secrets).await;
        client.close().await;
        result?;

        Ok(())
    }

    pub async fn repository_mirror_exists(
        &self,
        handle: RepositoryHandle,
        host: String,
    ) -> Result<bool, Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;

        let mut client = self.connect_remote_client(&host).await?;

        let result = client
            .mirror_exists(holder.repository().secrets().id())
            .await;
        client.close().await;

        Ok(result?)
    }

    pub async fn repository_quota(&self, handle: RepositoryHandle) -> Result<QuotaInfo, Error> {
        let holder = self.repos.get(handle).ok_or(Error::RepositoryNotFound)?;
        let quota = holder.repository().quota().await?;
        let size = holder.repository().size().await?;

        Ok(QuotaInfo { quota, size })
    }

    pub async fn set_repository_quota(
        &self,
        handle: RepositoryHandle,
        quota: Option<StorageSize>,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .repository()
            .set_quota(quota)
            .await?;

        Ok(())
    }

    pub async fn default_quota(&self) -> Result<Option<StorageSize>, Error> {
        Ok(ouisync_bridge::repository::get_default_quota(&self.config).await?)
    }

    pub async fn set_default_quota(&self, value: Option<StorageSize>) -> Result<(), Error> {
        ouisync_bridge::repository::set_default_quota(&self.config, value).await?;
        Ok(())
    }

    pub async fn set_repository_expiration(
        &self,
        handle: RepositoryHandle,
        value: Option<Duration>,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .set_repository_expiration(value)
            .await?;
        Ok(())
    }

    pub async fn repository_expiration(
        &self,
        handle: RepositoryHandle,
    ) -> Result<Option<Duration>, Error> {
        self.repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .repository_expiration()
            .await
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

    pub async fn set_block_expiration(
        &self,
        handle: RepositoryHandle,
        value: Option<Duration>,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .repository()
            .set_block_expiration(value)
            .await?;
        Ok(())
    }

    pub fn block_expiration(&self, handle: RepositoryHandle) -> Result<Option<Duration>, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::RepositoryNotFound)?
            .repository()
            .block_expiration())
    }

    pub async fn set_default_block_expiration(&self, value: Option<Duration>) -> Result<(), Error> {
        ouisync_bridge::repository::set_default_block_expiration(&self.config, value).await?;
        Ok(())
    }

    pub async fn default_block_expiration(&self) -> Result<Option<Duration>, Error> {
        Ok(ouisync_bridge::repository::get_default_block_expiration(&self.config).await?)
    }

    pub async fn delete_expired_repositories(&mut self) {
        let mut expired = Vec::new();

        for (handle, holder) in self.repos.iter() {
            let Some(last_block_expiration_time) = holder.repository().last_block_expiration_time()
            else {
                continue;
            };

            let expiration = match holder.repository_expiration().await {
                Ok(Some(duration)) => duration,
                Ok(None) => continue,
                Err(error) => {
                    tracing::error!(?error, "failed to get repository expiration");
                    continue;
                }
            };

            let elapsed = match last_block_expiration_time.elapsed() {
                Ok(duration) => duration,
                Err(error) => {
                    tracing::error!(
                        ?error,
                        "failed to compute elapsed time since last block expiration"
                    );
                    continue;
                }
            };

            if elapsed < expiration {
                continue;
            }

            expired.push(handle);
        }

        for handle in expired {
            match self.delete_repository(handle).await {
                Ok(()) => (),
                Err(error) => {
                    tracing::error!(?error, "failed to delete expired repository");
                }
            }
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

    pub async fn close(&mut self) {
        self.network.shutdown().await;
        self.close_repositories().await;
    }

    // Find all repositories in the store dir and open them.
    async fn load_repositories(&mut self) {
        if !fs::try_exists(&self.store_dir).await.unwrap_or(false) {
            tracing::error!("store dir doesn't exist");
            return;
        }

        let mut walkdir = utils::walk_dir(&self.store_dir);

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

    async fn close_repositories(&mut self) {
        for mut holder in self.repos.drain() {
            if let Err(error) = self.mounter.remove(holder.name()) {
                tracing::warn!(?error, repo = holder.name(), "failed to unmount repository",);
            }

            if let Err(error) = holder.close().await {
                tracing::warn!(?error, repo = holder.name(), "failed to close repository");
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

        let name = path
            .strip_prefix(&self.store_dir)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .into_owned();

        let registration = self.network.register(repo.handle()).await;

        let mut holder = RepositoryHolder::new(name, repo);
        holder.enable_sync(registration);

        if holder
            .repository()
            .metadata()
            .get(AUTOMOUNT_KEY)
            .await?
            .unwrap_or(false)
        {
            self.mounter
                .insert(holder.name().to_owned(), holder.repository().clone())?;
        }

        self.repos.try_insert(holder).ok_or(Error::RepositoryExists)
    }

    fn store_path(&self, repo_name: &str) -> PathBuf {
        let suffix = Path::new(repo_name);
        let extension = if let Some(extension) = suffix.extension() {
            let mut extension = extension.to_owned();
            extension.push(".");
            extension.push(REPOSITORY_FILE_EXTENSION);

            Cow::Owned(extension)
        } else {
            Cow::Borrowed(REPOSITORY_FILE_EXTENSION.as_ref())
        };

        self.store_dir.join(suffix).with_extension(extension)
    }

    async fn connect_remote_client(&self, host: &str) -> Result<RemoteClient, Error> {
        Ok(RemoteClient::connect(host, self.remote_client_config().await?).await?)
    }
}

async fn make_server_config(config_dir: &Path) -> Result<Arc<rustls::ServerConfig>, Error> {
    let cert_path = config_dir.join("cert.pem");
    let key_path = config_dir.join("key.pem");

    let certs = tls::load_certificates_from_file(&cert_path)
        .await
        .map_err(|error| {
            tracing::error!(
                "failed to load TLS certificate from {}: {}",
                cert_path.display(),
                error,
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

    let keys = tls::load_keys_from_file(&key_path).await.map_err(|error| {
        tracing::error!(
            "failed to load TLS key from {}: {}",
            key_path.display(),
            error
        );

        Error::TlsKeysNotFound
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn store_path_sanity_check() {
        let temp_dir = TempDir::new().unwrap();
        let store_dir = temp_dir.path().join("store");
        let state = State::init(
            temp_dir.path().join("config"),
            Defaults {
                store_dir: store_dir.clone(),
                mount_dir: temp_dir.path().join("mount"),
                bind: vec![],
                local_discovery_enabled: false,
                port_forwarding_enabled: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(state.store_path("foo"), store_dir.join("foo.ouisyncdb"));
        assert_eq!(
            state.store_path("foo/bar"),
            store_dir.join("foo/bar.ouisyncdb")
        );
        assert_eq!(
            state.store_path("foo/bar.baz"),
            store_dir.join("foo/bar.baz.ouisyncdb")
        );
    }

    /*
    use futures_util::TryStreamExt;
    use ouisync::{Access, AccessSecrets, PeerAddr, Repository, RepositoryParams, WriteSecrets};
    use std::net::Ipv4Addr;
    use tokio::fs;
    use tokio_stream::wrappers::ReadDirStream;
    use tracing::Instrument;

    #[tokio::test]
    async fn expire_empty_repository() {
        init_log();

        let base_dir = TempDir::new().unwrap();
        let secrets = WriteSecrets::random();

        let state = State::init(
            &Dirs {
                config_dir: base_dir.path().join("config"),
                store_dir: base_dir.path().join("store"),
                mount_dir: base_dir.path().join("mount"),
            },
            StateMonitor::make_root(),
        )
        .await
        .unwrap();

        let name = "foo";
        let holder = state
            .create_repository(
                CreateRepositoryMethod::Import {
                    share_token: ShareToken::from(AccessSecrets::Blind { id: secrets.id })
                        .with_name(name),
                },
                None,
                None,
            )
            .await
            .unwrap();

        // Repository expiration requires block expiration to be enabled as well.
        holder
            .repository
            .set_block_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();

        holder
            .set_repository_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();

        drop(holder);

        time::sleep(Duration::from_secs(1)).await;

        state.delete_expired_repositories().await;

        assert!(!state.repositories.contains(name));
        assert_eq!(
            read_dir(base_dir.path().join("store")).await,
            Vec::<PathBuf>::new()
        );
    }

    #[tokio::test]
    async fn expire_synced_repository() {
        init_log();

        let base_dir = TempDir::new().unwrap();

        let secrets = WriteSecrets::random();
        let monitor = StateMonitor::make_root();

        let (remote_network, _remote_repo, _remote_reg) = async {
            let monitor = monitor.make_child("remote");

            let repo = Repository::create(
                &RepositoryParams::new(base_dir.path().join("remote/repo.ouisyncdb"))
                    .with_parent_monitor(monitor.clone()),
                Access::WriteUnlocked {
                    secrets: secrets.clone(),
                },
            )
            .await
            .unwrap();

            let mut file = repo.create_file("test.txt").await.unwrap();
            file.write_all(b"hello world").await.unwrap();
            file.flush().await.unwrap();
            drop(file);

            let network = Network::new(monitor, None, None);
            network
                .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
                .await;

            let reg = network.register(repo.handle()).await;

            (network, repo, reg)
        }
        .instrument(tracing::info_span!("remote"))
        .await;

        let remote_addr = remote_network
            .listener_local_addrs()
            .into_iter()
            .next()
            .unwrap();

        let local_state = State::init(
            &Dirs {
                config_dir: base_dir.path().join("local/config"),
                store_dir: base_dir.path().join("local/store"),
                mount_dir: base_dir.path().join("local/mount"),
            },
            monitor.make_child("local"),
        )
        .instrument(tracing::info_span!("local"))
        .await
        .unwrap();

        local_state
            .network
            .bind(&[PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;
        local_state.network.add_user_provided_peer(&remote_addr);

        let name = "foo";
        let holder = local_state
            .create_repository(
                CreateRepositoryMethod::Import {
                    share_token: ShareToken::from(AccessSecrets::Blind { id: secrets.id })
                        .with_name(name),
                },
                None,
                None,
            )
            .await
            .unwrap();

        // Wait until synced
        let mut rx = holder.repository.subscribe();

        time::timeout(Duration::from_secs(30), async {
            loop {
                let progress = holder.repository.sync_progress().await.unwrap();

                if progress.total > 0 && progress.value == progress.total {
                    break;
                }

                rx.recv().await.unwrap();
            }
        })
        .await
        .unwrap();

        // Enable expiration
        holder
            .repository
            .set_block_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();
        holder
            .set_repository_expiration(Some(Duration::from_millis(100)))
            .await
            .unwrap();

        drop(holder);

        time::sleep(Duration::from_secs(1)).await;

        local_state.delete_expired_repositories().await;

        assert!(!local_state.repositories.contains(name));
        assert_eq!(
            read_dir(base_dir.path().join("local/store")).await,
            Vec::<PathBuf>::new()
        );
    }

    fn init_log() {
        tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_target(false)
            .with_test_writer()
            .try_init()
            .ok();
    }

    async fn read_dir(path: impl AsRef<Path>) -> Vec<PathBuf> {
        ReadDirStream::new(fs::read_dir(path).await.unwrap())
            .map_ok(|entry| entry.path())
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
    }
    */
}

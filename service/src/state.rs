use crate::{
    error::Error,
    file::{FileHandle, FileHolder, FileSet},
    protocol::{DirectoryEntry, ImportMode, MetadataEdit, QuotaInfo},
    repository::{FindError, RepositoryHandle, RepositoryHolder, RepositorySet},
    transport::remote::RemoteClient,
    utils,
};
use ouisync::{
    Access, AccessChange, AccessMode, AccessSecrets, Credentials, EntryType, Event, LocalSecret,
    Network, PeerAddr, Progress, Repository, RepositoryParams, SetLocalSecret, ShareToken,
    StorageSize,
};
use ouisync_bridge::{
    config::{ConfigError, ConfigKey, ConfigStore},
    network::{self, NetworkDefaults},
    transport::tls,
};
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use state_monitor::{MonitorId, StateMonitor};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    ffi::OsStr,
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs,
    sync::{broadcast, watch, OnceCell},
};
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
    mounter: Option<MultiRepoVFS>,
    repos: RepositorySet,
    files: FileSet,
    root_monitor: StateMonitor,
    repos_monitor: StateMonitor,
    remote_server_config: OnceCell<Arc<rustls::ServerConfig>>,
    remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
}

impl State {
    pub async fn init(config_dir: PathBuf, default_store_dir: PathBuf) -> Result<Self, Error> {
        let config = ConfigStore::new(config_dir);
        let root_monitor = StateMonitor::make_root();

        let network = Network::new(
            root_monitor.make_child("Network"),
            Some(config.dht_contacts_store()),
            None,
        );

        let store_dir = match config.entry(STORE_DIR_KEY).get().await {
            Ok(dir) => dir,
            Err(ConfigError::NotFound) => default_store_dir,
            Err(error) => return Err(error.into()),
        };

        let mount_dir = match config.entry(MOUNT_DIR_KEY).get().await {
            Ok(dir) => Some(dir),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        let mounter = if let Some(mount_dir) = mount_dir {
            Some(MultiRepoVFS::create(mount_dir).await?)
        } else {
            None
        };

        let repos_monitor = root_monitor.make_child("Repositories");

        let mut state = Self {
            config,
            network,
            store_dir,
            mounter,
            root_monitor,
            repos_monitor,
            repos: RepositorySet::new(),
            files: FileSet::new(),
            remote_server_config: OnceCell::new(),
            remote_client_config: OnceCell::new(),
        };

        state.load_repositories().await;

        Ok(state)
    }

    /// Initializes the network according to the stored configuration. If a particular network
    /// parameter is not yet configured, falls back to the given defaults.
    pub async fn init_network(&self, defaults: NetworkDefaults) {
        network::init(&self.network, &self.config, defaults).await;
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

    pub fn mount_dir(&self) -> Option<&Path> {
        self.mounter.as_ref().map(|m| m.mount_root())
    }

    pub async fn set_mount_dir(&mut self, dir: PathBuf) -> Result<(), Error> {
        if Some(dir.as_path()) == self.mount_dir() {
            return Ok(());
        }

        self.config.entry(MOUNT_DIR_KEY).set(&dir).await?;
        let mounter = self.mounter.insert(MultiRepoVFS::create(dir).await?);

        // Remount all mounted repos
        for (_, holder) in self.repos.iter() {
            if holder
                .repository()
                .metadata()
                .get(AUTOMOUNT_KEY)
                .await?
                .unwrap_or(false)
            {
                mounter.insert(holder.name().to_owned(), holder.repository().clone())?;
            }
        }

        Ok(())
    }

    pub fn find_repository(&self, name: &str) -> Result<RepositoryHandle, FindError> {
        let (handle, _) = self.repos.find_by_name(name)?;
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
        if self.repos.find_by_name(&name).is_ok() {
            Err(Error::RepositoryExists)?;
        }

        if let Some(token) = &share_token {
            if self.repos.find_by_id(token.id()).is_some() {
                Err(Error::RepositoryExists)?;
            }
        }

        let store_path = self.store_path(name.as_ref());

        let params = RepositoryParams::new(store_path)
            .with_device_id(ouisync_bridge::device_id::get_or_create(&self.config).await?)
            .with_monitor(self.repos_monitor.make_child(name.clone()));

        let access_secrets = if let Some(share_token) = share_token {
            share_token.into_secrets()
        } else {
            AccessSecrets::random_write()
        };

        let access = Access::new(read_secret, write_secret, access_secrets);

        let repo = Repository::create(&params, access).await?;

        let value = ouisync_bridge::repository::get_default_quota(&self.config).await?;
        repo.set_quota(value).await?;

        let value = ouisync_bridge::repository::get_default_block_expiration(&self.config).await?;
        repo.set_block_expiration(value).await?;

        let registration = self.network.register(repo.handle()).await;
        registration.set_dht_enabled(enable_dht).await;
        registration.set_pex_enabled(enable_pex).await;

        let mut holder = RepositoryHolder::new(name.clone(), repo);
        holder.enable_sync(registration);

        let value = self.default_repository_expiration().await?;
        holder.set_repository_expiration(value).await?;

        // unwrap is ok because we already checked that the repo doesn't exist earlier and we have
        // exclusive access to this state.
        let handle = self.repos.try_insert(holder).unwrap();

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

    pub async fn open_repository(
        &mut self,
        name: String,
        local_secret: Option<LocalSecret>,
    ) -> Result<RepositoryHandle, Error> {
        if self.repos.find_by_name(&name).is_ok() {
            // TODO: return the existing repo instead (but unlock using `local_secret`)?
            Err(Error::RepositoryExists)?;
        }

        let store_path = self.store_path(&name);

        self.load_repository(&store_path, local_secret).await
    }

    pub async fn close_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let mut holder = self.repos.remove(handle).ok_or(Error::InvalidArgument)?;

        if let Some(mounter) = &self.mounter {
            mounter.remove(holder.name())?;
        }

        holder.close().await?;

        Ok(())
    }

    pub async fn export_repository(
        &self,
        handle: RepositoryHandle,
        output_path: PathBuf,
    ) -> Result<PathBuf, Error> {
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;

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
                if let Ok((handle, _)) = self.repos.find_by_name(&name) {
                    if let Some(mut holder) = self.repos.remove(handle) {
                        holder.close().await?;
                    }
                }
            } else {
                return Err(Error::RepositoryExists);
            }
        }

        // TODO: check if repo with same id exists

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

        self.load_repository(&store_path, None).await
    }

    pub async fn share_repository(
        &self,
        handle: RepositoryHandle,
        secret: Option<LocalSecret>,
        mode: AccessMode,
    ) -> Result<ShareToken, Error> {
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;
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
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_credentials(new_credentials)
            .await?;

        Ok(())
    }

    pub async fn set_repository_access(
        &self,
        handle: RepositoryHandle,
        read: Option<AccessChange>,
        write: Option<AccessChange>,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_access(read, write)
            .await?;
        Ok(())
    }

    pub fn repository_access_mode(&self, handle: RepositoryHandle) -> Result<AccessMode, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .access_mode())
    }

    pub async fn set_repository_access_mode(
        &self,
        handle: RepositoryHandle,
        access_mode: AccessMode,
        local_secret: Option<LocalSecret>,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_access_mode(access_mode, local_secret)
            .await?;

        Ok(())
    }

    pub fn repository_credentials(&self, handle: RepositoryHandle) -> Result<Vec<u8>, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .credentials()
            .encode())
    }

    pub async fn set_repository_credentials(
        &self,
        handle: RepositoryHandle,
        credentials: Vec<u8>,
    ) -> Result<(), Error> {
        self.repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_credentials(Credentials::decode(&credentials)?)
            .await?;

        Ok(())
    }

    pub async fn mount_repository(&mut self, handle: RepositoryHandle) -> Result<PathBuf, Error> {
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;

        holder
            .repository()
            .metadata()
            .set(AUTOMOUNT_KEY, true)
            .await?;

        let Some(mounter) = &self.mounter else {
            return Err(Error::OperationNotSupported);
        };

        if let Some(mount_point) = mounter.mount_point(holder.name()) {
            // Already mounted
            Ok(mount_point)
        } else {
            Ok(mounter.insert(holder.name().to_owned(), holder.repository().clone())?)
        }
    }

    pub async fn unmount_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;

        holder.repository().metadata().remove(AUTOMOUNT_KEY).await?;

        if let Some(mounter) = &self.mounter {
            mounter.remove(holder.name())?;
        }

        Ok(())
    }

    pub fn subscribe_to_repository(
        &self,
        handle: RepositoryHandle,
    ) -> Result<broadcast::Receiver<Event>, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .subscribe())
    }

    pub fn is_repository_sync_enabled(&self, handle: RepositoryHandle) -> Result<bool, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .registration()
            .is_some())
    }

    pub async fn set_repository_sync_enabled(
        &mut self,
        handle: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        let holder = self.repos.get_mut(handle).ok_or(Error::InvalidArgument)?;

        match (enabled, holder.registration().is_some()) {
            (true, false) => {
                let registration = self.network.register(holder.repository().handle()).await;
                holder.enable_sync(registration);
            }
            (false, true) => {
                holder.disable_sync();
            }
            (true, true) | (false, false) => (),
        }

        Ok(())
    }

    pub async fn repository_sync_progress(
        &self,
        handle: RepositoryHandle,
    ) -> Result<Progress, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .sync_progress()
            .await?)
    }

    pub fn is_repository_dht_enabled(&self, handle: RepositoryHandle) -> Result<bool, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
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
            .ok_or(Error::InvalidArgument)?
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
            .ok_or(Error::InvalidArgument)?
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
            .ok_or(Error::InvalidArgument)?
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
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;
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
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;
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
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;

        let mut client = self.connect_remote_client(&host).await?;

        let result = client
            .mirror_exists(holder.repository().secrets().id())
            .await;
        client.close().await;

        Ok(result?)
    }

    pub async fn repository_quota(&self, handle: RepositoryHandle) -> Result<QuotaInfo, Error> {
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;
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
            .ok_or(Error::InvalidArgument)?
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
            .ok_or(Error::InvalidArgument)?
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
            .ok_or(Error::InvalidArgument)?
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
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_block_expiration(value)
            .await?;
        Ok(())
    }

    pub fn block_expiration(&self, handle: RepositoryHandle) -> Result<Option<Duration>, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
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

    pub async fn repository_metadata(
        &self,
        handle: RepositoryHandle,
        key: String,
    ) -> Result<Option<String>, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .metadata()
            .get(&key)
            .await?)
    }

    pub async fn set_repository_metadata(
        &self,
        handle: RepositoryHandle,
        edits: Vec<MetadataEdit>,
    ) -> Result<bool, Error> {
        let repo = self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository();

        let mut tx = repo.metadata().write().await?;

        for edit in edits {
            if tx.get(&edit.key).await? != edit.old {
                return Ok(false);
            }

            if let Some(new) = edit.new {
                tx.set(&edit.key, new).await?;
            } else {
                tx.remove(&edit.key).await?;
            }
        }

        tx.commit().await?;

        Ok(true)
    }

    /// Returns the type of repository entry (file, directory, ...) or `None` if the entry doesn't
    /// exist.
    pub async fn repository_entry_type(
        &self,
        handle: RepositoryHandle,
        path: String,
    ) -> Result<Option<EntryType>, Error> {
        match self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .lookup_type(path)
            .await
        {
            Ok(entry_type) => Ok(Some(entry_type)),
            Err(ouisync::Error::EntryNotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn move_repository_entry(
        &self,
        repo: RepositoryHandle,
        src: String,
        dst: String,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .move_entry(src, dst)
            .await?;

        Ok(())
    }

    pub async fn create_directory(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .create_directory(path)
            .await?;

        Ok(())
    }

    pub async fn read_directory(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<Vec<DirectoryEntry>, Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();

        let dir = repo.open_directory(path).await?;
        let entries = dir
            .entries()
            .map(|entry| DirectoryEntry {
                name: entry.unique_name().into_owned(),
                entry_type: entry.entry_type(),
            })
            .collect();

        Ok(entries)
    }

    /// Removes the directory at the given path from the repository. If `recursive` is true it removes
    /// also the contents, otherwise the directory must be empty.
    pub async fn remove_directory(
        &self,
        repo: RepositoryHandle,
        path: String,
        recursive: bool,
    ) -> Result<(), Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();

        if recursive {
            repo.remove_entry_recursively(path).await?
        } else {
            repo.remove_entry(path).await?
        }

        Ok(())
    }

    pub async fn create_file(
        &mut self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<FileHandle, Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();
        let local_branch = repo.local_branch()?;

        let file = repo.create_file(&path).await?;
        let holder = FileHolder { file, local_branch };
        let handle = self.files.insert(holder);

        Ok(handle)
    }

    pub async fn open_file(
        &mut self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<FileHandle, Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();
        let local_branch = repo.local_branch()?;

        let file = repo.open_file(&path).await?;
        let holder = FileHolder { file, local_branch };
        let handle = self.files.insert(holder);

        Ok(handle)
    }

    /// Remove (delete) the file at the given path from the repository.
    pub async fn remove_file(&mut self, repo: RepositoryHandle, path: String) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .remove_entry(&path)
            .await?;
        Ok(())
    }

    pub async fn read_file(
        &mut self,
        handle: FileHandle,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, Error> {
        let len = len as usize;
        let mut buffer = vec![0; len];

        let holder = self.files.get_mut(handle).ok_or(Error::InvalidArgument)?;

        holder.file.seek(SeekFrom::Start(offset));

        // TODO: consider using just `read`
        let len = holder.file.read_all(&mut buffer).await?;
        buffer.truncate(len);

        Ok(buffer)
    }

    pub async fn write_file(
        &mut self,
        file: FileHandle,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        let holder = self.files.get_mut(file).ok_or(Error::InvalidArgument)?;

        holder.file.seek(SeekFrom::Start(offset));
        holder.file.fork(holder.local_branch.clone()).await?;

        // TODO: consider using just `write` and returning the number of bytes written
        holder.file.write_all(&data).await?;

        Ok(())
    }

    pub async fn file_exists(&self, repo: RepositoryHandle, path: String) -> Result<bool, Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();

        match repo.lookup_type(&path).await {
            Ok(EntryType::File) => Ok(true),
            Ok(EntryType::Directory) => Ok(false),
            Err(ouisync::Error::EntryNotFound) => Ok(false),
            Err(ouisync::Error::AmbiguousEntry) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn file_len(&self, handle: FileHandle) -> Result<u64, Error> {
        Ok(self
            .files
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .file
            .len())
    }

    pub async fn file_progress(&self, handle: FileHandle) -> Result<u64, Error> {
        // Don't keep the file locked while progress is being awaited.
        let progress = self
            .files
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .file
            .progress();
        let progress = progress.await?;

        Ok(progress)
    }

    pub async fn truncate_file(&mut self, handle: FileHandle, len: u64) -> Result<(), Error> {
        let holder = self.files.get_mut(handle).ok_or(Error::InvalidArgument)?;
        holder.file.fork(holder.local_branch.clone()).await?;
        holder.file.truncate(len)?;

        Ok(())
    }

    pub async fn flush_file(&mut self, handle: FileHandle) -> Result<(), Error> {
        self.files
            .get_mut(handle)
            .ok_or(Error::InvalidArgument)?
            .file
            .flush()
            .await?;

        Ok(())
    }

    pub async fn close_file(&mut self, handle: FileHandle) -> Result<(), Error> {
        if let Some(mut holder) = self.files.remove(handle) {
            holder.file.flush().await?
        }

        Ok(())
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

    pub fn state_monitor(&self, path: Vec<MonitorId>) -> Result<StateMonitor, Error> {
        self.root_monitor.locate(path).ok_or(Error::InvalidArgument)
    }

    pub fn subscribe_to_state_monitor(
        &self,
        path: Vec<MonitorId>,
    ) -> Result<watch::Receiver<()>, Error> {
        Ok(self
            .root_monitor
            .locate(path)
            .ok_or(Error::InvalidArgument)?
            .subscribe())
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

            match self.load_repository(path, None).await {
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
            if let Some(mounter) = &self.mounter {
                if let Err(error) = mounter.remove(holder.name()) {
                    tracing::warn!(?error, repo = holder.name(), "failed to unmount repository",);
                }
            }

            if let Err(error) = holder.close().await {
                tracing::warn!(?error, repo = holder.name(), "failed to close repository");
            }
        }
    }

    async fn load_repository(
        &mut self,
        path: &Path,
        local_secret: Option<LocalSecret>,
    ) -> Result<RepositoryHandle, Error> {
        let name = path
            .strip_prefix(&self.store_dir)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .into_owned();

        let params = RepositoryParams::new(path)
            .with_device_id(ouisync_bridge::device_id::get_or_create(&self.config).await?)
            .with_monitor(self.repos_monitor.make_child(name.clone()));

        let repo = Repository::open(&params, local_secret, AccessMode::Write).await?;
        let registration = self.network.register(repo.handle()).await;

        let mut holder = RepositoryHolder::new(name, repo);
        holder.enable_sync(registration);

        if let Some(mounter) = &self.mounter {
            if holder
                .repository()
                .metadata()
                .get(AUTOMOUNT_KEY)
                .await?
                .unwrap_or(false)
            {
                mounter.insert(holder.name().to_owned(), holder.repository().clone())?;
            }
        }

        self.repos.try_insert(holder).ok_or(Error::RepositoryExists)
    }

    fn store_path(&self, repo_name: &str) -> PathBuf {
        // TODO: when `repo_name` is already a path (starts with '/' for absolute or './' for
        // relative), use it as is

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

    ouisync_bridge::transport::make_server_config(certs, key).map_err(Error::TlsConfig)
}

async fn make_client_config(config_dir: &Path) -> Result<Arc<rustls::ClientConfig>, Error> {
    // Load custom root certificates (if any)
    let additional_root_certs = tls::load_certificates_from_dir(&config_dir.join("root_certs"))
        .await
        .map_err(Error::TlsCertificatesInvalid)?;

    ouisync_bridge::transport::make_client_config(&additional_root_certs).map_err(Error::TlsConfig)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use crate::test_utils;

    use super::*;
    use assert_matches::assert_matches;
    use futures_util::TryStreamExt;
    use ouisync::{Access, AccessSecrets, Repository, RepositoryParams, WriteSecrets};
    use tempfile::TempDir;
    use tokio::time;
    use tokio_stream::wrappers::ReadDirStream;
    use tracing::Instrument;

    #[tokio::test]
    async fn store_path_sanity_check() {
        let (_temp_dir, state) = setup().await;
        let store_dir = state.store_dir();

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

    #[tokio::test]
    async fn non_unique_repository_name() {
        let (_temp_dir, mut state) = setup().await;
        let name = "test";

        assert_matches!(
            state
                .create_repository(name.to_owned(), None, None, None, false, false)
                .await,
            Ok(_)
        );

        assert_matches!(
            state
                .create_repository(name.to_owned(), None, None, None, false, false)
                .await,
            Err(Error::RepositoryExists)
        );
    }

    #[tokio::test]
    async fn non_unique_repository_id() {
        let (_temp_dir, mut state) = setup().await;

        let token = ShareToken::from(AccessSecrets::Write(WriteSecrets::random()));

        assert_matches!(
            state
                .create_repository(
                    "foo".to_owned(),
                    None,
                    None,
                    Some(token.clone()),
                    false,
                    false
                )
                .await,
            Ok(_)
        );

        // different name but same token
        assert_matches!(
            state
                .create_repository("bar".to_owned(), None, None, Some(token), false, false)
                .await,
            Err(Error::RepositoryExists)
        );
    }

    // TODO: test import repo with non-unique id

    #[tokio::test]
    async fn expire_empty_repository() {
        test_utils::init_log();

        let (_temp_dir, mut state) = setup().await;

        let secrets = WriteSecrets::random();

        let name = "foo";
        let handle = state
            .create_repository(
                name.to_owned(),
                None,
                None,
                Some(ShareToken::from(AccessSecrets::Blind { id: secrets.id })),
                false,
                false,
            )
            .await
            .unwrap();

        // Repository expiration requires block expiration to be enabled as well.
        state
            .set_block_expiration(handle, Some(Duration::from_millis(100)))
            .await
            .unwrap();

        state
            .set_repository_expiration(handle, Some(Duration::from_millis(100)))
            .await
            .unwrap();

        time::sleep(Duration::from_secs(1)).await;

        state.delete_expired_repositories().await;

        assert_eq!(state.find_repository(name), Err(FindError::NotFound));
        assert_eq!(read_dir(state.store_dir()).await, Vec::<PathBuf>::new());
    }

    #[tokio::test]
    async fn expire_synced_repository() {
        test_utils::init_log();

        let temp_dir = TempDir::new().unwrap();

        let secrets = WriteSecrets::random();

        let (remote_network, _remote_repo, _remote_reg) = async {
            let monitor = StateMonitor::make_root();

            let repo = Repository::create(
                &RepositoryParams::new(temp_dir.path().join("remote/repo.ouisyncdb"))
                    .with_monitor(monitor.make_child("repo")),
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

        let mut local_state = State::init(
            temp_dir.path().join("local/config"),
            temp_dir.path().join("local/store"),
        )
        .instrument(tracing::info_span!("local"))
        .await
        .unwrap();

        local_state.set_port_forwarding_enabled(false).await;
        local_state.set_local_discovery_enabled(false).await;
        local_state
            .bind_network(vec![PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())])
            .await;
        local_state.network.add_user_provided_peer(&remote_addr);

        let name = "foo";
        let handle = local_state
            .create_repository(
                name.to_owned(),
                None,
                None,
                Some(ShareToken::from(AccessSecrets::Blind { id: secrets.id })),
                false,
                false,
            )
            .await
            .unwrap();
        let local_repo = local_state.repos.get(handle).unwrap().repository();

        // Wait until synced
        let mut rx = local_repo.subscribe();

        time::timeout(Duration::from_secs(30), async {
            loop {
                let progress = local_repo.sync_progress().await.unwrap();

                if progress.total > 0 && progress.value == progress.total {
                    break;
                }

                rx.recv().await.unwrap();
            }
        })
        .await
        .unwrap();

        // Enable expiration
        local_state
            .set_block_expiration(handle, Some(Duration::from_millis(100)))
            .await
            .unwrap();
        local_state
            .set_repository_expiration(handle, Some(Duration::from_millis(100)))
            .await
            .unwrap();

        time::sleep(Duration::from_secs(1)).await;

        local_state.delete_expired_repositories().await;

        assert_eq!(local_state.find_repository(name), Err(FindError::NotFound));
        assert_eq!(
            read_dir(local_state.store_dir()).await,
            Vec::<PathBuf>::new(),
        );
    }

    async fn setup() -> (TempDir, State) {
        let temp_dir = TempDir::new().unwrap();
        let state = State::init(
            temp_dir.path().join("config"),
            temp_dir.path().join("store"),
        )
        .await
        .unwrap();

        state.bind_network(vec![]).await;
        state.set_port_forwarding_enabled(false).await;
        state.set_local_discovery_enabled(false).await;

        (temp_dir, state)
    }

    async fn read_dir(path: impl AsRef<Path>) -> Vec<PathBuf> {
        ReadDirStream::new(fs::read_dir(path).await.unwrap())
            .map_ok(|entry| entry.path())
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
    }
}

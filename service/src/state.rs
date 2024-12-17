use crate::{
    error::Error,
    file::{FileHandle, FileHolder, FileSet},
    network::PexConfig,
    protocol::{DirectoryEntry, ImportMode, MetadataEdit, NetworkDefaults, QuotaInfo},
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
    network,
    transport::tls,
};
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use state_monitor::{MonitorId, StateMonitor};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    ffi::OsStr,
    io::{self, SeekFrom},
    panic,
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

#[cfg(test)]
mod tests;

// Global configs

const BIND_KEY: ConfigKey<Vec<PeerAddr>> =
    ConfigKey::new("bind", "Addresses to bind the network listeners to");

const LOCAL_DISCOVERY_ENABLED_KEY: ConfigKey<bool> =
    ConfigKey::new("local_discovery_enabled", "Enable local discovery");

const MOUNT_DIR_KEY: ConfigKey<PathBuf> = ConfigKey::new("mount_dir", "Repository mount directory");

const PEERS_KEY: ConfigKey<Vec<PeerAddr>> = ConfigKey::new(
    "peers",
    "List of peers to connect to in addition to the ones found by various discovery mechanisms\n\
     (e.g. DHT)",
);

const PEX_KEY: ConfigKey<PexConfig> = ConfigKey::new("pex", "Peer exchange configuration");

const PORT_FORWARDING_ENABLED_KEY: ConfigKey<bool> =
    ConfigKey::new("port_forwarding_enabled", "Enable port forwarding / UPnP");

const STORE_DIR_KEY: ConfigKey<PathBuf> =
    ConfigKey::new("store_dir", "Repository storage directory");

const DEFAULT_REPOSITORY_EXPIRATION_KEY: ConfigKey<u64> = ConfigKey::new(
    "default_repository_expiration",
    "Default time in milliseconds after repository is deleted if all its blocks expired",
);

// Per-repo configs

const AUTOMOUNT_KEY: &str = "automount";
const DHT_ENABLED_KEY: &str = "dht_enabled";
const PEX_ENABLED_KEY: &str = "pex_enabled";

// Other constants

const REPOSITORY_FILE_EXTENSION: &str = "ouisyncdb";

pub(crate) struct State {
    pub config: ConfigStore,
    pub network: Network,
    store_dir: Option<PathBuf>,
    mounter: Option<MultiRepoVFS>,
    repos: RepositorySet,
    files: FileSet,
    root_monitor: StateMonitor,
    repos_monitor: StateMonitor,
    remote_server_config: OnceCell<Arc<rustls::ServerConfig>>,
    remote_client_config: OnceCell<Arc<rustls::ClientConfig>>,
}

impl State {
    pub async fn init(config_dir: PathBuf) -> Result<Self, Error> {
        let config = ConfigStore::new(config_dir);
        let root_monitor = StateMonitor::make_root();

        let network = Network::new(
            root_monitor.make_child("Network"),
            Some(config.dht_contacts_store()),
            None,
        );

        let store_dir = match config.entry(STORE_DIR_KEY).get().await {
            Ok(dir) => Some(dir),
            Err(ConfigError::NotFound) => None,
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

    pub fn enable_panic_monitor(&self) {
        let panic_counter = self
            .root_monitor
            .make_child("Service")
            .make_value("panic_counter", 0u32);

        // TODO: This is not atomic. Use `panic::update_hook` when stabilized.
        let default_panic_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            *panic_counter.get() += 1;
            default_panic_hook(panic_info);
        }));
    }

    /// Initializes the network according to the stored configuration. If a particular network
    /// parameter is not yet configured, falls back to the given defaults.
    pub async fn init_network(&self, defaults: NetworkDefaults) {
        let bind_addrs = self
            .config
            .entry(BIND_KEY)
            .get()
            .await
            .unwrap_or(defaults.bind);
        ouisync_bridge::network::bind_with_reuse_ports(&self.network, &self.config, &bind_addrs)
            .await;

        let enabled = self
            .config
            .entry(PORT_FORWARDING_ENABLED_KEY)
            .get()
            .await
            .unwrap_or(defaults.port_forwarding_enabled);
        self.network.set_port_forwarding_enabled(enabled);

        let enabled = self
            .config
            .entry(LOCAL_DISCOVERY_ENABLED_KEY)
            .get()
            .await
            .unwrap_or(defaults.local_discovery_enabled);
        self.network.set_local_discovery_enabled(enabled);

        let peers = self.config.entry(PEERS_KEY).get().await.unwrap_or_default();
        for peer in peers {
            self.network.add_user_provided_peer(&peer);
        }

        let PexConfig { send, recv } = self.config.entry(PEX_KEY).get().await.unwrap_or_default();
        self.network.set_pex_send_enabled(send);
        self.network.set_pex_recv_enabled(recv);
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

    pub fn store_dir(&self) -> Option<&Path> {
        self.store_dir.as_deref()
    }

    pub async fn set_store_dir(&mut self, dir: PathBuf) -> Result<(), Error> {
        if Some(dir.as_path()) == self.store_dir.as_deref() {
            return Ok(());
        }

        self.config.entry(STORE_DIR_KEY).set(&dir).await?;
        self.store_dir = Some(dir);

        // Close repos from the previous store dir and load repos from the new dir.
        self.close_repositories().await;
        self.load_repositories().await;

        Ok(())
    }

    pub fn mount_root(&self) -> Option<&Path> {
        self.mounter.as_ref().map(|m| m.mount_root())
    }

    pub async fn set_mount_root(&mut self, dir: Option<PathBuf>) -> Result<(), Error> {
        if dir.as_deref() == self.mount_root() {
            return Ok(());
        }

        let config_entry = self.config.entry(MOUNT_DIR_KEY);

        if let Some(dir) = dir {
            config_entry.set(&dir).await?;
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
                    mounter.insert(holder.short_name().to_owned(), holder.repository().clone())?;
                }
            }
        } else {
            config_entry.remove().await?;
            self.mounter = None;
        }

        Ok(())
    }

    pub fn find_repository(&self, pattern: &str) -> Result<RepositoryHandle, FindError> {
        let (handle, _) = self.repos.find_by_subpath(pattern)?;
        Ok(handle)
    }

    pub fn list_repositories(&self) -> BTreeMap<PathBuf, RepositoryHandle> {
        self.repos
            .iter()
            .map(|(handle, holder)| (holder.path().to_owned(), handle))
            .collect()
    }

    #[expect(clippy::too_many_arguments)] // TODO: extract the args to a struct
    pub async fn create_repository(
        &mut self,
        path: &Path,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        share_token: Option<ShareToken>,
        sync_enabled: bool,
        dht_enabled: bool,
        pex_enabled: bool,
    ) -> Result<RepositoryHandle, Error> {
        let path = self.normalize_repository_path(path)?;

        if self.repos.find_by_path(&path).is_some() {
            Err(Error::RepositoryExists)?;
        }

        if let Some(token) = &share_token {
            if self.repos.find_by_id(token.id()).is_some() {
                Err(Error::RepositoryExists)?;
            }
        }

        // Create the repo
        let params = RepositoryParams::new(&path)
            .with_device_id(ouisync_bridge::device_id::get_or_create(&self.config).await?)
            .with_monitor(self.repos_monitor.make_child(path.to_string_lossy()));

        let access_secrets = if let Some(share_token) = share_token {
            share_token.into_secrets()
        } else {
            AccessSecrets::random_write()
        };

        let access = Access::new(read_secret, write_secret, access_secrets);

        let repo = Repository::create(&params, access).await?;
        let mut holder = RepositoryHolder::new(path, repo);

        // Configure syncing
        if sync_enabled {
            let registration = self.network.register(holder.repository().handle());
            registration.set_dht_enabled(dht_enabled);
            registration.set_pex_enabled(pex_enabled);
            holder.enable_sync(registration);
        }

        if dht_enabled || pex_enabled {
            let metadata = holder.repository().metadata();
            let mut writer = metadata.write().await?;

            if dht_enabled {
                writer.set(DHT_ENABLED_KEY, true).await?;
            }

            if pex_enabled {
                writer.set(PEX_ENABLED_KEY, true).await?;
            }

            writer.commit().await?;
        }

        // Configure quota and expiration
        let value = ouisync_bridge::repository::get_default_quota(&self.config).await?;
        holder.repository().set_quota(value).await?;

        let value = ouisync_bridge::repository::get_default_block_expiration(&self.config).await?;
        holder.repository().set_block_expiration(value).await?;

        let value = self.default_repository_expiration().await?;
        holder.set_repository_expiration(value).await?;

        tracing::info!(name = holder.short_name(), "repository created");

        // unwrap is ok because we already checked that the repo doesn't exist earlier and we have
        // exclusive access to this state.
        let handle = self.repos.try_insert(holder).unwrap();

        Ok(handle)
    }

    pub async fn delete_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let Some(mut holder) = self.repos.remove(handle) else {
            return Ok(());
        };

        holder.close().await?;

        for path in ouisync::repository_files(holder.path()) {
            match fs::remove_file(path).await {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }

        self.remove_empty_ancestor_dirs(holder.path()).await?;

        tracing::info!(name = holder.short_name(), "repository deleted");

        Ok(())
    }

    pub async fn open_repository(
        &mut self,
        path: &Path,
        local_secret: Option<LocalSecret>,
    ) -> Result<RepositoryHandle, Error> {
        let path = self.normalize_repository_path(path)?;
        let handle = if let Some((handle, holder)) = self.repos.find_by_path(&path) {
            // If `local_secret` provides higher access mode than what the repo currently has,
            // increase it. If not, the access mode remains unchanged.
            holder
                .repository()
                .set_access_mode(AccessMode::Write, local_secret)
                .await?;

            handle
        } else {
            let holder = self.load_repository(&path, local_secret, false).await?;
            // unwrap is ok because we already handled the case when the repo already exists.
            self.repos.try_insert(holder).unwrap()
        };

        Ok(handle)
    }

    pub async fn close_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let mut holder = self.repos.remove(handle).ok_or(Error::InvalidArgument)?;

        if let Some(mounter) = &self.mounter {
            mounter.remove(holder.short_name())?;
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

    #[deprecated = "use open_repository or move/copy/link the file manually"]
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

        let path = self.normalize_repository_path(Path::new(&name))?;

        if fs::try_exists(&path).await? {
            if force {
                if let Some((handle, _)) = self.repos.find_by_path(&path) {
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
                fs::copy(input_path, &path).await?;
            }
            ImportMode::Move => {
                fs::rename(input_path, &path).await?;
            }
            ImportMode::SoftLink => {
                #[cfg(unix)]
                fs::symlink(input_path, &path).await?;

                #[cfg(windows)]
                fs::symlink_file(input_path, &path).await?;

                #[cfg(not(any(unix, windows)))]
                return Err(Error::OperationNotSupported);
            }
            ImportMode::HardLink => {
                fs::hard_link(input_path, &path).await?;
            }
        }

        let holder = self.load_repository(&path, None, true).await?;
        self.repos.try_insert(holder).ok_or(Error::RepositoryExists)
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
            Some(holder.short_name().to_owned()),
        )
        .await?;

        Ok(token)
    }

    pub fn repository_path(&self, handle: RepositoryHandle) -> Result<PathBuf, Error> {
        Ok(self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .path()
            .to_owned())
    }

    /// Return the info-hash of the repository formatted as hex string. This can be used as a globally
    /// unique, non-secret identifier of the repository.
    /// User is responsible for deallocating the returned string.
    pub fn repository_info_hash(&self, handle: RepositoryHandle) -> Result<String, Error> {
        let repo = self
            .repos
            .get(handle)
            .ok_or(Error::InvalidArgument)?
            .repository();
        let info_hash = ouisync::repository_info_hash(repo.secrets().id());

        Ok(hex::encode(info_hash))
    }

    pub async fn move_repository(
        &mut self,
        handle: RepositoryHandle,
        dst: &Path,
    ) -> Result<(), Error> {
        // This function is "best effort atomic".

        let dst = self.normalize_repository_path(dst)?;
        let dst_parent = dst.parent().ok_or(Error::InvalidArgument)?;

        if self.repos.find_by_path(&dst).is_some() {
            return Err(Error::RepositoryExists);
        }

        let holder = self.repos.get_mut(handle).ok_or(Error::InvalidArgument)?;
        // Preserve access mode after the move
        let credentials = holder.repository().credentials();
        let sync_enabled = holder.registration().is_some();

        // TODO: close all open files of this repo

        if let Some(mounter) = &self.mounter {
            mounter.remove(holder.short_name())?;
        }

        fs::create_dir_all(dst_parent).await?;

        if let Err(error) = holder.close().await {
            // Try to revert creating the dst parent directory but if it fails still return the
            // close error because it's more important.
            self.remove_empty_ancestor_dirs(dst_parent).await.ok();
            return Err(error);
        }

        let src = holder.path().to_owned();

        let (old_path, new_path, result) = match move_file(&src, &dst).await {
            Ok(()) => (src, dst, Ok(())),
            Err(error) => (dst, src, Err(error.into())), // Restore the original repo
        };

        *holder = load_repository(
            &new_path,
            None,
            sync_enabled,
            &self.config,
            &self.network,
            &self.repos_monitor,
            self.mounter.as_ref(),
        )
        .await?;
        holder.repository().set_credentials(credentials).await?;

        self.remove_empty_ancestor_dirs(&old_path).await?;

        result
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

        if let Some(mount_point) = mounter.mount_point(holder.short_name()) {
            // Already mounted
            Ok(mount_point)
        } else {
            Ok(mounter.insert(holder.short_name().to_owned(), holder.repository().clone())?)
        }
    }

    pub async fn unmount_repository(&mut self, handle: RepositoryHandle) -> Result<(), Error> {
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;

        holder.repository().metadata().remove(AUTOMOUNT_KEY).await?;

        if let Some(mounter) = &self.mounter {
            mounter.remove(holder.short_name())?;
        }

        Ok(())
    }

    pub fn repository_mount_point(
        &self,
        handle: RepositoryHandle,
    ) -> Result<Option<PathBuf>, Error> {
        if let Some(mounter) = &self.mounter {
            Ok(mounter.mount_point(
                self.repos
                    .get(handle)
                    .ok_or(Error::InvalidArgument)?
                    .short_name(),
            ))
        } else {
            Ok(None)
        }
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
        set_sync_enabled(holder, &self.network, enabled).await?;

        Ok(())
    }

    pub async fn set_all_repositories_sync_enabled(&mut self, enabled: bool) -> Result<(), Error> {
        for (_, holder) in self.repos.iter_mut() {
            set_sync_enabled(holder, &self.network, enabled).await?;
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
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;

        if let Some(reg) = holder.registration() {
            reg.set_dht_enabled(enabled);
        }

        set_metadata_bool(holder.repository(), DHT_ENABLED_KEY, enabled).await?;

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
        let holder = self.repos.get(handle).ok_or(Error::InvalidArgument)?;

        if let Some(reg) = holder.registration() {
            reg.set_pex_enabled(enabled);
        }

        set_metadata_bool(holder.repository(), PEX_ENABLED_KEY, enabled).await?;

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
        path: &str,
    ) -> Result<FileHandle, Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();
        let local_branch = repo.local_branch()?;

        let file = repo.open_file(path).await?;
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
        data: &[u8],
    ) -> Result<(), Error> {
        let holder = self.files.get_mut(file).ok_or(Error::InvalidArgument)?;

        holder.file.seek(SeekFrom::Start(offset));
        holder.file.fork(holder.local_branch.clone()).await?;

        // TODO: consider using just `write` and returning the number of bytes written
        holder.file.write_all(data).await?;

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
        let Some(store_dir) = self.store_dir.as_deref() else {
            tracing::warn!("store dir not specified");
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

            match self.load_repository(path, None, false).await {
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
                if let Err(error) = mounter.remove(holder.short_name()) {
                    tracing::warn!(
                        ?error,
                        repo = holder.short_name(),
                        "failed to unmount repository",
                    );
                }
            }

            if let Err(error) = holder.close().await {
                tracing::warn!(
                    ?error,
                    repo = holder.short_name(),
                    "failed to close repository"
                );
            }
        }
    }

    async fn load_repository(
        &self,
        path: &Path,
        local_secret: Option<LocalSecret>,
        sync_enabled: bool,
    ) -> Result<RepositoryHolder, Error> {
        load_repository(
            path,
            local_secret,
            sync_enabled,
            &self.config,
            &self.network,
            &self.repos_monitor,
            self.mounter.as_ref(),
        )
        .await
    }

    fn normalize_repository_path(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = if path.is_absolute() {
            Cow::Borrowed(path)
        } else {
            Cow::Owned(
                self.store_dir
                    .as_deref()
                    .ok_or(Error::StoreDirUnspecified)?
                    .join(path),
            )
        };

        match path.extension() {
            Some(extension) if extension == REPOSITORY_FILE_EXTENSION => Ok(path.into_owned()),
            Some(extension) => {
                let mut extension = extension.to_owned();
                extension.push(".");
                extension.push(REPOSITORY_FILE_EXTENSION);

                Ok(path.with_extension(extension))
            }
            None => Ok(path.with_extension(REPOSITORY_FILE_EXTENSION)),
        }
    }

    // Remove ancestors directories up to `store_dir` but only if they are empty.
    async fn remove_empty_ancestor_dirs(&self, path: &Path) -> Result<(), io::Error> {
        let Some(store_dir) = &self.store_dir else {
            return Ok(());
        };

        if !path.starts_with(store_dir) {
            return Ok(());
        }

        for path in path.ancestors().skip(1) {
            if path == store_dir {
                break;
            }

            match fs::remove_dir(&path).await {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => break,
                Err(error) => return Err(error),
            }
        }

        Ok(())
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

async fn load_repository(
    path: &Path,
    local_secret: Option<LocalSecret>,
    sync_enabled: bool,
    config: &ConfigStore,
    network: &Network,
    repos_monitor: &StateMonitor,
    mounter: Option<&MultiRepoVFS>,
) -> Result<RepositoryHolder, Error> {
    let params = RepositoryParams::new(path)
        .with_device_id(ouisync_bridge::device_id::get_or_create(config).await?)
        .with_monitor(repos_monitor.make_child(path.to_string_lossy()));

    let repo = Repository::open(&params, local_secret, AccessMode::Write).await?;
    let mut holder = RepositoryHolder::new(path.to_owned(), repo);

    if sync_enabled {
        set_sync_enabled(&mut holder, network, true).await?;
    }

    if let Some(mounter) = &mounter {
        if holder
            .repository()
            .metadata()
            .get(AUTOMOUNT_KEY)
            .await?
            .unwrap_or(false)
        {
            mounter.insert(holder.short_name().to_owned(), holder.repository().clone())?;
        }
    }

    Ok(holder)
}

async fn set_sync_enabled(
    holder: &mut RepositoryHolder,
    network: &Network,
    enabled: bool,
) -> Result<(), Error> {
    match (enabled, holder.registration().is_some()) {
        (true, false) => {
            let registration = network.register(holder.repository().handle());

            let metadata = holder.repository().metadata();
            registration.set_dht_enabled(metadata.get(DHT_ENABLED_KEY).await?.unwrap_or(false));
            registration.set_pex_enabled(metadata.get(PEX_ENABLED_KEY).await?.unwrap_or(false));

            holder.enable_sync(registration);
        }
        (false, true) => {
            holder.disable_sync();
        }
        (true, true) | (false, false) => (),
    }

    Ok(())
}

async fn set_metadata_bool(repo: &Repository, key: &str, value: bool) -> Result<(), Error> {
    if value {
        repo.metadata().set(key, true).await?;
    } else {
        repo.metadata().remove(key).await?;
    }

    Ok(())
}

/// Move file from `src` to `dst`. If they are on the same filesystem, it does a simple rename.
/// Otherwise it copies `src` to `dst` first and then deletes `src`.
async fn move_file(src: &Path, dst: &Path) -> io::Result<()> {
    // First try rename
    match fs::rename(src, dst).await {
        Ok(()) => return Ok(()),
        Err(_error) => {
            // TODO: we should only fallback on `io::ErrorKind::CrossesDevices` but that variant is
            // currently unstable.
        }
    }

    // If that didn't work, fallback to copy + remove
    fs::copy(src, dst).await?;
    fs::remove_file(src).await?;

    Ok(())
}

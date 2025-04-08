mod move_repository;
#[cfg(test)]
mod tests;

use crate::{
    config_keys::{
        BIND_KEY, DEFAULT_BLOCK_EXPIRATION_MILLIS, DEFAULT_QUOTA_KEY,
        DEFAULT_REPOSITORY_EXPIRATION_KEY, LOCAL_DISCOVERY_ENABLED_KEY, MOUNT_DIR_KEY, PEERS_KEY,
        PEX_KEY, PORT_FORWARDING_ENABLED_KEY, STORE_DIR_KEY,
    },
    config_store::{ConfigError, ConfigStore},
    device_id, dht_contacts,
    error::Error,
    file::{FileHandle, FileHolder, FileSet},
    network::{self, PexConfig},
    protocol::{DirectoryEntry, MetadataEdit, NetworkDefaults, QuotaInfo},
    repository::{RepositoryHandle, RepositoryHolder, RepositorySet},
    tls,
    transport::remote::RemoteClient,
    utils,
};
use ouisync::{
    crypto::{cipher::SecretKey, Password, PasswordSalt},
    Access, AccessChange, AccessMode, AccessSecrets, Credentials, EntryType, Event, LocalSecret,
    NatBehavior, Network, PeerAddr, PeerInfo, Progress, PublicRuntimeId, Repository,
    RepositoryParams, SetLocalSecret, ShareToken, Stats, StorageSize,
};
use ouisync_macros::api;
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use rand::{rngs::OsRng, Rng};
use state_monitor::{MonitorId, StateMonitor};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    ffi::OsStr,
    io::{self, SeekFrom},
    net::SocketAddr,
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

const AUTOMOUNT_KEY: &str = "automount";
const DHT_ENABLED_KEY: &str = "dht_enabled";
const PEX_ENABLED_KEY: &str = "pex_enabled";

const REPOSITORY_FILE_EXTENSION: &str = "ouisyncdb";

pub(crate) struct State {
    pub config: ConfigStore,
    pub network: Network,
    store: Store,
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
        let dht_contacts_store = dht_contacts::Store::new(config.dir());

        let network = Network::new(
            root_monitor.make_child("Network"),
            Some(Arc::new(dht_contacts_store)),
            None,
        );

        let store_dir = match config.entry(STORE_DIR_KEY).get().await {
            Ok(dir) => Some(dir),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        let store = Store { dir: store_dir };

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
            store,
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
    #[api]
    pub async fn session_init_network(&self, defaults: NetworkDefaults) {
        let bind_addrs = self
            .config
            .entry(BIND_KEY)
            .get()
            .await
            .unwrap_or(defaults.bind);
        network::bind_with_reuse_ports(&self.network, &self.config, &bind_addrs).await;

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

    #[api]
    pub async fn session_bind_network(&self, addrs: Vec<PeerAddr>) {
        self.config.entry(BIND_KEY).set(&addrs).await.ok();
        network::bind_with_reuse_ports(&self.network, &self.config, &addrs).await;
    }

    #[api]
    pub fn session_get_current_protocol_version(&self) -> u64 {
        self.network.current_protocol_version()
    }

    #[api]
    pub fn session_get_highest_seen_protocol_version(&self) -> u64 {
        self.network.highest_seen_protocol_version()
    }

    #[api]
    pub async fn session_get_external_addr_v4(&self) -> Option<SocketAddr> {
        self.network.external_addr_v4().await.map(SocketAddr::V4)
    }

    #[api]
    pub async fn session_get_external_addr_v6(&self) -> Option<SocketAddr> {
        self.network.external_addr_v6().await.map(SocketAddr::V6)
    }

    #[api]
    pub fn session_get_local_listener_addrs(&self) -> Vec<PeerAddr> {
        self.network.listener_local_addrs()
    }

    #[api]
    pub async fn session_get_remote_listener_addrs(
        &self,
        host: String,
    ) -> Result<Vec<PeerAddr>, Error> {
        let mut client = self.connect_remote_client(&host).await?;
        let result = client.get_listener_addrs().await;
        client.close().await;

        Ok(result?)
    }

    #[api]
    pub async fn session_get_nat_behavior(&self) -> Option<NatBehavior> {
        self.network.nat_behavior().await
    }

    #[api]
    pub fn session_get_peers(&self) -> Vec<PeerInfo> {
        self.network.peer_info_collector().collect()
    }

    #[api]
    pub async fn session_add_user_provided_peers(&self, addrs: Vec<PeerAddr>) {
        let entry = self.config.entry(PEERS_KEY);
        let mut stored = entry.get().await.unwrap_or_default();

        let len = stored.len();
        stored.extend(addrs.iter().copied());
        stored.sort();
        stored.dedup();

        if stored.len() > len {
            entry.set(&stored).await.ok();
        }

        for addr in &addrs {
            self.network.add_user_provided_peer(addr);
        }
    }

    #[api]
    pub async fn session_remove_user_provided_peers(&self, addrs: Vec<PeerAddr>) {
        let entry = self.config.entry(PEERS_KEY);
        let mut stored = entry.get().await.unwrap_or_default();

        let len = stored.len();
        stored.retain(|stored| !addrs.contains(stored));

        if stored.len() < len {
            entry.set(&stored).await.ok();
        }

        for addr in &addrs {
            self.network.remove_user_provided_peer(addr);
        }
    }

    #[api]
    pub async fn session_get_user_provided_peers(&self) -> Vec<PeerAddr> {
        self.config.entry(PEERS_KEY).get().await.unwrap_or_default()
    }

    #[api]
    pub fn session_is_local_discovery_enabled(&self) -> bool {
        self.network.is_local_discovery_enabled()
    }

    #[api]
    pub async fn session_set_local_discovery_enabled(&self, enabled: bool) {
        self.config
            .entry(LOCAL_DISCOVERY_ENABLED_KEY)
            .set(&enabled)
            .await
            .ok();
        self.network.set_local_discovery_enabled(enabled);
    }

    #[api]
    pub fn session_is_port_forwarding_enabled(&self) -> bool {
        self.network.is_port_forwarding_enabled()
    }

    #[api]
    pub async fn session_set_port_forwarding_enabled(&self, enabled: bool) {
        self.config
            .entry(PORT_FORWARDING_ENABLED_KEY)
            .set(&enabled)
            .await
            .ok();
        self.network.set_port_forwarding_enabled(enabled);
    }

    /// Checks whether accepting peers discovered on the peer exchange is enabled.
    #[api]
    pub fn session_is_pex_recv_enabled(&self) -> bool {
        self.network.is_pex_recv_enabled()
    }

    #[api]
    pub async fn session_set_pex_recv_enabled(&self, enabled: bool) {
        self.config
            .entry(PEX_KEY)
            .modify(|pex_config| pex_config.recv = enabled)
            .await
            .ok();

        self.network.set_pex_recv_enabled(enabled);
    }

    #[api]
    pub fn session_is_pex_send_enabled(&self) -> bool {
        self.network.is_pex_send_enabled()
    }

    #[api]
    pub async fn session_set_pex_send_enabled(&self, enabled: bool) {
        self.config
            .entry(PEX_KEY)
            .modify(|pex_config| pex_config.send = enabled)
            .await
            .ok();

        self.network.set_pex_send_enabled(enabled);
    }

    #[api]
    pub fn session_get_runtime_id(&self) -> PublicRuntimeId {
        self.network.this_runtime_id()
    }

    #[api]
    pub fn session_get_network_stats(&self) -> Stats {
        self.network.stats()
    }

    #[api]
    pub fn session_get_store_dir(&self) -> Option<PathBuf> {
        self.store_dir().map(|path| path.to_owned())
    }

    #[api]
    pub async fn session_set_store_dir(&mut self, path: PathBuf) -> Result<(), Error> {
        if Some(path.as_path()) == self.store.dir.as_deref() {
            return Ok(());
        }

        self.config.entry(STORE_DIR_KEY).set(&path).await?;
        self.store.dir = Some(path);

        // Close repos from the previous store dir and load repos from the new dir.
        self.close_repositories().await;
        self.load_repositories().await;

        Ok(())
    }

    #[api]
    pub fn session_get_mount_root(&self) -> Option<PathBuf> {
        self.mount_root().map(|path| path.to_owned())
    }

    #[api]
    pub async fn session_set_mount_root(&mut self, path: Option<PathBuf>) -> Result<(), Error> {
        if path.as_deref() == self.mount_root() {
            return Ok(());
        }

        let config_entry = self.config.entry(MOUNT_DIR_KEY);

        if let Some(path) = path {
            config_entry.set(&path).await?;
            let mounter = self.mounter.insert(MultiRepoVFS::create(path).await?);

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

    #[api]
    pub fn session_validate_share_token(&self, token: String) -> Result<ShareToken, Error> {
        Ok(token.parse()?)
    }

    /// Return the info-hash of the repository corresponding to the given token, formatted as hex
    /// string.
    ///
    /// See also: [repository_get_info_hash]
    #[api]
    pub fn session_get_share_token_info_hash(&self, token: ShareToken) -> String {
        hex::encode(ouisync::repository_info_hash(token.id()).as_ref())
    }

    #[api]
    pub fn session_get_share_token_access_mode(&self, token: ShareToken) -> AccessMode {
        token.access_mode()
    }

    #[api]
    pub fn session_get_share_token_suggested_name(&self, token: ShareToken) -> String {
        token.suggested_name().to_owned()
    }

    #[api]
    pub async fn session_mirror_exists(
        &self,
        token: ShareToken,
        host: String,
    ) -> Result<bool, Error> {
        let mut client = self.connect_remote_client(&host).await?;

        let result = client.mirror_exists(token.id()).await;
        client.close().await;

        Ok(result?)
    }

    #[api]
    pub fn session_find_repository(&self, name: String) -> Result<RepositoryHandle, Error> {
        let (handle, _) = self.repos.find_by_subpath(&name)?;
        Ok(handle)
    }

    #[api]
    pub fn session_list_repositories(&self) -> BTreeMap<PathBuf, RepositoryHandle> {
        self.repos
            .iter()
            .map(|(handle, holder)| (holder.path().to_owned(), handle))
            .collect()
    }

    #[api]
    #[expect(clippy::too_many_arguments)] // TODO: extract the args to a struct
    pub async fn session_create_repository(
        &mut self,
        path: PathBuf,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        token: Option<ShareToken>,
        sync_enabled: bool,
        dht_enabled: bool,
        pex_enabled: bool,
    ) -> Result<RepositoryHandle, Error> {
        let path = self.store.normalize_repository_path(&path)?;

        if self.repos.find_by_path(&path).is_some() {
            Err(Error::AlreadyExists)?;
        }

        if let Some(token) = &token {
            if self.repos.find_by_id(token.id()).is_some() {
                Err(Error::AlreadyExists)?;
            }
        }

        // Create the repo
        let params = RepositoryParams::new(&path)
            .with_device_id(device_id::get_or_create(&self.config).await?)
            .with_monitor(self.repos_monitor.make_child(path.to_string_lossy()));

        let access_secrets = if let Some(token) = token {
            token.into_secrets()
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
        let value = default_quota(&self.config).await?;
        holder.repository().set_quota(value).await?;

        let value = default_block_expiration(&self.config).await?;
        holder.repository().set_block_expiration(value).await?;

        let value = self.session_get_default_repository_expiration().await?;
        holder.set_repository_expiration(value).await?;

        tracing::info!(name = holder.short_name(), "repository created");

        // unwrap is ok because we already checked that the repo doesn't exist earlier and we have
        // exclusive access to this state.
        let handle = self.repos.try_insert(holder).unwrap();

        Ok(handle)
    }

    /// Delete a repository with the given name.
    #[api]
    pub async fn session_delete_repository_by_name(&mut self, name: String) -> Result<(), Error> {
        let (handle, _) = self.repos.find_by_subpath(&name)?;
        self.repository_delete(handle).await?;
        Ok(())
    }

    #[api]
    pub async fn session_open_repository(
        &mut self,
        path: PathBuf,
        local_secret: Option<LocalSecret>,
    ) -> Result<RepositoryHandle, Error> {
        let path = self.store.normalize_repository_path(&path)?;
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

    /// Delete a repository
    #[api]
    pub async fn repository_delete(&mut self, repo: RepositoryHandle) -> Result<(), Error> {
        let Some(mut holder) = self.repos.remove(repo) else {
            return Ok(());
        };

        holder.close().await?;

        for path in ouisync::database_files(holder.path()) {
            match fs::remove_file(path).await {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        }

        self.store.remove_empty_ancestor_dirs(holder.path()).await?;

        tracing::info!(name = holder.short_name(), "repository deleted");

        Ok(())
    }

    #[api]
    pub async fn repository_close(&mut self, repo: RepositoryHandle) -> Result<(), Error> {
        let mut holder = self.repos.remove(repo).ok_or(Error::InvalidArgument)?;

        if let Some(mounter) = &self.mounter {
            mounter.remove(holder.short_name())?;
        }

        holder.close().await?;

        Ok(())
    }

    /// Export repository to file
    #[api]
    pub async fn repository_export(
        &self,
        repo: RepositoryHandle,
        output_path: PathBuf,
    ) -> Result<PathBuf, Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        let output_path = if output_path.extension().is_some() {
            output_path
        } else {
            output_path.with_extension(REPOSITORY_FILE_EXTENSION)
        };

        holder.repository().export(&output_path).await?;

        Ok(output_path)
    }

    #[api]
    pub async fn repository_share(
        &self,
        repo: RepositoryHandle,
        local_secret: Option<LocalSecret>,
        access_mode: AccessMode,
    ) -> Result<ShareToken, Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        let access_secrets = if let Some(local_secret) = local_secret {
            holder.repository().unlock_secrets(local_secret).await?
        } else {
            holder.repository().secrets()
        };

        let share_token =
            ShareToken::from(access_secrets.with_mode(access_mode)).with_name(holder.short_name());

        Ok(share_token)
    }

    #[api]
    pub fn repository_get_path(&self, repo: RepositoryHandle) -> Result<PathBuf, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .path()
            .to_owned())
    }

    /// Return the info-hash of the repository formatted as hex string. This can be used as a globally
    /// unique, non-secret identifier of the repository.
    #[api]
    pub fn repository_get_info_hash(&self, repo: RepositoryHandle) -> Result<String, Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();
        let info_hash = ouisync::repository_info_hash(repo.secrets().id());

        Ok(hex::encode(info_hash))
    }

    #[api]
    pub async fn repository_move(
        &mut self,
        repo: RepositoryHandle,
        dst: PathBuf,
    ) -> Result<(), Error> {
        move_repository::invoke(self, repo, &dst).await
    }

    #[api]
    pub async fn repository_reset_access(
        &self,
        repo: RepositoryHandle,
        token: ShareToken,
    ) -> Result<(), Error> {
        let new_credentials = Credentials::with_random_writer_id(token.into_secrets());
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_credentials(new_credentials)
            .await?;

        Ok(())
    }

    #[api]
    pub async fn repository_set_access(
        &self,
        repo: RepositoryHandle,
        read: Option<AccessChange>,
        write: Option<AccessChange>,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_access(read, write)
            .await?;
        Ok(())
    }

    #[api]
    pub fn repository_get_access_mode(&self, repo: RepositoryHandle) -> Result<AccessMode, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .access_mode())
    }

    #[api]
    pub async fn repository_set_access_mode(
        &self,
        repo: RepositoryHandle,
        access_mode: AccessMode,
        local_secret: Option<LocalSecret>,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_access_mode(access_mode, local_secret)
            .await?;

        Ok(())
    }

    #[api]
    pub fn repository_get_credentials(&self, repo: RepositoryHandle) -> Result<Vec<u8>, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .credentials()
            .encode())
    }

    #[api]
    pub async fn repository_set_credentials(
        &self,
        repo: RepositoryHandle,
        credentials: Vec<u8>,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_credentials(Credentials::decode(&credentials)?)
            .await?;

        Ok(())
    }

    #[api]
    pub async fn repository_mount(&mut self, repo: RepositoryHandle) -> Result<PathBuf, Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

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

    #[api]
    pub async fn repository_unmount(&mut self, repo: RepositoryHandle) -> Result<(), Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        holder.repository().metadata().remove(AUTOMOUNT_KEY).await?;

        if let Some(mounter) = &self.mounter {
            mounter.remove(holder.short_name())?;
        }

        Ok(())
    }

    #[api]
    pub fn repository_get_mount_point(
        &self,
        repo: RepositoryHandle,
    ) -> Result<Option<PathBuf>, Error> {
        if let Some(mounter) = &self.mounter {
            Ok(mounter.mount_point(
                self.repos
                    .get(repo)
                    .ok_or(Error::InvalidArgument)?
                    .short_name(),
            ))
        } else {
            Ok(None)
        }
    }

    pub fn repository_subscribe(
        &self,
        repo: RepositoryHandle,
    ) -> Result<broadcast::Receiver<Event>, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .subscribe())
    }

    #[api]
    pub fn repository_is_sync_enabled(&self, repo: RepositoryHandle) -> Result<bool, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .registration()
            .is_some())
    }

    #[api]
    pub async fn repository_set_sync_enabled(
        &mut self,
        repo: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        let holder = self.repos.get_mut(repo).ok_or(Error::InvalidArgument)?;
        set_sync_enabled(holder, &self.network, enabled).await?;

        Ok(())
    }

    #[api]
    pub async fn repository_get_sync_progress(
        &self,
        repo: RepositoryHandle,
    ) -> Result<Progress, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .sync_progress()
            .await?)
    }

    #[api]
    pub async fn repository_is_dht_enabled(&self, repo: RepositoryHandle) -> Result<bool, Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        if let Some(reg) = holder.registration() {
            Ok(reg.is_dht_enabled())
        } else {
            Ok(holder
                .repository()
                .metadata()
                .get(DHT_ENABLED_KEY)
                .await?
                .unwrap_or(false))
        }
    }

    #[api]
    pub async fn repository_set_dht_enabled(
        &self,
        repo: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        if let Some(reg) = holder.registration() {
            reg.set_dht_enabled(enabled);
        }

        set_metadata_bool(holder.repository(), DHT_ENABLED_KEY, enabled).await?;

        Ok(())
    }

    #[api]
    pub async fn repository_is_pex_enabled(&self, repo: RepositoryHandle) -> Result<bool, Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        if let Some(reg) = holder.registration() {
            Ok(reg.is_pex_enabled())
        } else {
            Ok(holder
                .repository()
                .metadata()
                .get(PEX_ENABLED_KEY)
                .await?
                .unwrap_or(false))
        }
    }

    #[api]
    pub async fn repository_set_pex_enabled(
        &self,
        repo: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        if let Some(reg) = holder.registration() {
            reg.set_pex_enabled(enabled);
        }

        set_metadata_bool(holder.repository(), PEX_ENABLED_KEY, enabled).await?;

        Ok(())
    }

    #[api]
    pub fn repository_get_stats(&self, repo: RepositoryHandle) -> Result<Stats, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .registration()
            .as_ref()
            .map(|reg| reg.stats())
            .unwrap_or_default())
    }

    #[api]
    pub async fn repository_create_mirror(
        &self,
        repo: RepositoryHandle,
        host: String,
    ) -> Result<(), Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;
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

    #[api]
    pub async fn repository_delete_mirror(
        &self,
        repo: RepositoryHandle,
        host: String,
    ) -> Result<(), Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;
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

    #[api]
    pub async fn repository_mirror_exists(
        &self,
        repo: RepositoryHandle,
        host: String,
    ) -> Result<bool, Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;

        let mut client = self.connect_remote_client(&host).await?;

        let result = client
            .mirror_exists(holder.repository().secrets().id())
            .await;
        client.close().await;

        Ok(result?)
    }

    #[api]
    pub async fn repository_get_quota(&self, repo: RepositoryHandle) -> Result<QuotaInfo, Error> {
        let holder = self.repos.get(repo).ok_or(Error::InvalidArgument)?;
        let quota = holder.repository().quota().await?;
        let size = holder.repository().size().await?;

        Ok(QuotaInfo { quota, size })
    }

    #[api]
    pub async fn repository_set_quota(
        &self,
        repo: RepositoryHandle,
        value: Option<StorageSize>,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_quota(value)
            .await?;

        Ok(())
    }

    #[api]
    pub async fn session_get_default_quota(&self) -> Result<Option<StorageSize>, Error> {
        Ok(default_quota(&self.config).await?)
    }

    #[api]
    pub async fn session_set_default_quota(&self, value: Option<StorageSize>) -> Result<(), Error> {
        set_default_quota(&self.config, value).await?;
        Ok(())
    }

    #[api]
    pub async fn repository_set_expiration(
        &self,
        repo: RepositoryHandle,
        value: Option<Duration>,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .set_repository_expiration(value)
            .await?;
        Ok(())
    }

    #[api]
    pub async fn repository_get_expiration(
        &self,
        repo: RepositoryHandle,
    ) -> Result<Option<Duration>, Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository_expiration()
            .await
    }

    #[api]
    pub async fn session_set_default_repository_expiration(
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

    #[api]
    pub async fn session_get_default_repository_expiration(
        &self,
    ) -> Result<Option<Duration>, Error> {
        let entry = self.config.entry::<u64>(DEFAULT_REPOSITORY_EXPIRATION_KEY);

        match entry.get().await {
            Ok(millis) => Ok(Some(Duration::from_millis(millis))),
            Err(ConfigError::NotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    #[api]
    pub async fn repository_set_block_expiration(
        &self,
        repo: RepositoryHandle,
        value: Option<Duration>,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .set_block_expiration(value)
            .await?;
        Ok(())
    }

    #[api]
    pub fn repository_get_block_expiration(
        &self,
        repo: RepositoryHandle,
    ) -> Result<Option<Duration>, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .block_expiration())
    }

    #[api]
    pub async fn session_set_default_block_expiration(
        &self,
        value: Option<Duration>,
    ) -> Result<(), Error> {
        set_default_block_expiration(&self.config, value).await?;
        Ok(())
    }

    #[api]
    pub async fn session_get_default_block_expiration(&self) -> Result<Option<Duration>, Error> {
        Ok(default_block_expiration(&self.config).await?)
    }

    #[api]
    pub async fn repository_get_metadata(
        &self,
        repo: RepositoryHandle,
        key: String,
    ) -> Result<Option<String>, Error> {
        Ok(self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .metadata()
            .get(&key)
            .await?)
    }

    #[api]
    pub async fn repository_set_metadata(
        &self,
        repo: RepositoryHandle,
        edits: Vec<MetadataEdit>,
    ) -> Result<bool, Error> {
        let repo = self
            .repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository();

        let mut tx = repo.metadata().write().await?;

        for edit in edits {
            if tx.get(&edit.key).await? != edit.old_value {
                return Ok(false);
            }

            if let Some(new) = edit.new_value {
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
    #[api]
    pub async fn repository_get_entry_type(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<Option<EntryType>, Error> {
        match self
            .repos
            .get(repo)
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

    #[api]
    pub async fn repository_move_entry(
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

    #[api]
    pub async fn repository_create_directory(
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

    #[api]
    pub async fn repository_read_directory(
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
    #[api]
    pub async fn repository_remove_directory(
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

    #[api]
    pub async fn repository_create_file(
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

    #[api]
    pub async fn repository_open_file(
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

        let file = repo.open_file(path).await?;
        let holder = FileHolder { file, local_branch };
        let handle = self.files.insert(holder);

        Ok(handle)
    }

    /// Remove (delete) the file at the given path from the repository.
    #[api]
    pub async fn repository_remove_file(
        &mut self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<(), Error> {
        self.repos
            .get(repo)
            .ok_or(Error::InvalidArgument)?
            .repository()
            .remove_entry(&path)
            .await?;
        Ok(())
    }

    #[api]
    pub async fn repository_file_exists(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<bool, Error> {
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

    /// Reads `size` bytes from the file starting at `offset` bytes from the beginning of the file.
    #[api]
    pub async fn file_read(
        &mut self,
        file: FileHandle,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, Error> {
        let size = size as usize;
        let mut buffer = vec![0; size];

        let holder = self.files.get_mut(file).ok_or(Error::InvalidArgument)?;

        holder.file.seek(SeekFrom::Start(offset));

        // TODO: consider using just `read`
        let size = holder.file.read_all(&mut buffer).await?;
        buffer.truncate(size);

        Ok(buffer)
    }

    #[api]
    pub async fn file_write(
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

    #[api]
    pub fn file_get_length(&self, file: FileHandle) -> Result<u64, Error> {
        Ok(self
            .files
            .get(file)
            .ok_or(Error::InvalidArgument)?
            .file
            .len())
    }

    /// Returns sync progress of the given file.
    #[api]
    pub async fn file_get_progress(&self, file: FileHandle) -> Result<u64, Error> {
        // Don't keep the file locked while progress is being awaited.
        let progress = self
            .files
            .get(file)
            .ok_or(Error::InvalidArgument)?
            .file
            .progress();
        let progress = progress.await?;

        Ok(progress)
    }

    #[api]
    pub async fn file_truncate(&mut self, file: FileHandle, len: u64) -> Result<(), Error> {
        let holder = self.files.get_mut(file).ok_or(Error::InvalidArgument)?;
        holder.file.fork(holder.local_branch.clone()).await?;
        holder.file.truncate(len)?;

        Ok(())
    }

    #[api]
    pub async fn file_flush(&mut self, file: FileHandle) -> Result<(), Error> {
        self.files
            .get_mut(file)
            .ok_or(Error::InvalidArgument)?
            .file
            .flush()
            .await?;

        Ok(())
    }

    #[api]
    pub async fn file_close(&mut self, file: FileHandle) -> Result<(), Error> {
        if let Some(mut holder) = self.files.remove(file) {
            holder.file.flush().await?
        }

        Ok(())
    }

    #[api]
    pub fn session_generate_password_salt(&self) -> PasswordSalt {
        OsRng.gen()
    }

    #[api]
    pub fn session_derive_secret_key(&self, password: Password, salt: PasswordSalt) -> SecretKey {
        SecretKey::derive_from_password(password.as_ref(), &salt)
    }

    #[api]
    pub fn session_generate_secret_key(&self) -> SecretKey {
        SecretKey::random()
    }

    #[api]
    pub fn session_get_state_monitor(&self, path: Vec<MonitorId>) -> Option<StateMonitor> {
        self.root_monitor.locate(path)
    }

    // This is exposed to #[api] in `Service`
    pub fn state_monitor_subscribe(
        &self,
        path: Vec<MonitorId>,
    ) -> Result<watch::Receiver<()>, Error> {
        Ok(self
            .root_monitor
            .locate(path)
            .ok_or(Error::NotFound)?
            .subscribe())
    }

    pub async fn set_all_repositories_sync_enabled(&mut self, enabled: bool) -> Result<(), Error> {
        for (_, holder) in self.repos.iter_mut() {
            set_sync_enabled(holder, &self.network, enabled).await?;
        }

        Ok(())
    }

    pub async fn remote_server_config(&self) -> Result<Arc<rustls::ServerConfig>, Error> {
        self.remote_server_config
            .get_or_try_init(|| tls::make_server_config(self.config.dir()))
            .await
            .cloned()
    }

    pub async fn remote_client_config(&self) -> Result<Arc<rustls::ClientConfig>, Error> {
        self.remote_client_config
            .get_or_try_init(|| tls::make_client_config(self.config.dir()))
            .await
            .cloned()
    }

    pub fn store_dir(&self) -> Option<&Path> {
        self.store.dir.as_deref()
    }

    pub fn mount_root(&self) -> Option<&Path> {
        self.mounter.as_ref().map(|m| m.mount_root())
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
            match self.repository_delete(handle).await {
                Ok(()) => (),
                Err(error) => {
                    tracing::error!(?error, "failed to delete expired repository");
                }
            }
        }
    }

    pub async fn close(&mut self) {
        self.network.shutdown().await;
        self.close_repositories().await;
    }

    // Find all repositories in the store dir and open them.
    async fn load_repositories(&mut self) {
        let Some(store_dir) = self.store.dir.as_deref() else {
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

            let holder = match self.load_repository(path, None, false).await {
                Ok(holder) => holder,
                Err(error) => {
                    tracing::error!(?error, ?path, "failed to open repository");
                    continue;
                }
            };

            if self.repos.try_insert(holder).is_none() {
                tracing::error!(?path, "repository already exists");
                continue;
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

    async fn connect_remote_client(&self, host: &str) -> Result<RemoteClient, Error> {
        Ok(RemoteClient::connect(host, self.remote_client_config().await?).await?)
    }
}

struct Store {
    dir: Option<PathBuf>,
}

impl Store {
    fn normalize_repository_path(&self, path: &Path) -> Result<PathBuf, Error> {
        let path = if path.is_absolute() {
            Cow::Borrowed(path)
        } else {
            Cow::Owned(
                self.dir
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
        let Some(store_dir) = &self.dir else {
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
        .with_device_id(device_id::get_or_create(config).await?)
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

async fn set_default_quota(
    config: &ConfigStore,
    value: Option<StorageSize>,
) -> Result<(), ConfigError> {
    let entry = config.entry(DEFAULT_QUOTA_KEY);

    if let Some(value) = value {
        entry.set(&value.to_bytes()).await?;
    } else {
        entry.remove().await?;
    }

    Ok(())
}

async fn default_quota(config: &ConfigStore) -> Result<Option<StorageSize>, ConfigError> {
    let entry = config.entry(DEFAULT_QUOTA_KEY);

    match entry.get().await {
        Ok(quota) => Ok(Some(StorageSize::from_bytes(quota))),
        Err(ConfigError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn set_default_block_expiration(
    config: &ConfigStore,
    value: Option<Duration>,
) -> Result<(), ConfigError> {
    let entry = config.entry(DEFAULT_BLOCK_EXPIRATION_MILLIS);

    if let Some(value) = value {
        entry
            .set(&value.as_millis().try_into().unwrap_or(u64::MAX))
            .await?;
    } else {
        entry.remove().await?;
    }

    Ok(())
}

async fn default_block_expiration(config: &ConfigStore) -> Result<Option<Duration>, ConfigError> {
    let entry = config.entry::<u64>(DEFAULT_BLOCK_EXPIRATION_MILLIS);

    match entry.get().await {
        Ok(millis) => Ok(Some(Duration::from_millis(millis))),
        Err(ConfigError::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

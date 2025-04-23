mod move_repository;
#[cfg(test)]
mod tests;

use crate::{
    config_keys::{
        BIND_KEY, DEFAULT_BLOCK_EXPIRATION_MILLIS, DEFAULT_QUOTA_KEY,
        DEFAULT_REPOSITORY_EXPIRATION_KEY, LOCAL_DISCOVERY_ENABLED_KEY, MOUNT_DIR_KEY, PEERS_KEY,
        PEX_KEY, PORT_FORWARDING_ENABLED_KEY, STORE_DIR_KEY,
    },
    config_store::{ConfigError, ConfigKey, ConfigStore},
    device_id, dht_contacts,
    error::Error,
    file::{FileHandle, FileHolder, FileSet},
    metrics::MetricsServer,
    network::{self, PexConfig},
    protocol::{DirectoryEntry, MessageId, MetadataEdit, NetworkDefaults, QuotaInfo},
    repository::{self, RepositoryHandle, RepositoryHolder, RepositorySet},
    tls::TlsConfig,
    transport::remote::{AcceptedRemoteConnection, RemoteClient, RemoteServer},
    utils,
};
use ouisync::{
    crypto::{cipher::SecretKey, Password, PasswordSalt},
    Access, AccessChange, AccessMode, AccessSecrets, Credentials, EntryType, Event, LocalSecret,
    NatBehavior, Network, NetworkEventReceiver, PeerAddr, PeerInfo, Progress, PublicRuntimeId,
    Registration, Repository, RepositoryParams, SetLocalSecret, ShareToken, Stats, StorageSize,
};
use ouisync_macros::api;
use ouisync_vfs::{MultiRepoMount, MultiRepoVFS};
use rand::{rngs::OsRng, Rng};
use state_monitor::{MonitorId, StateMonitor};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    ffi::OsStr,
    future,
    io::{self, SeekFrom},
    net::SocketAddr,
    panic,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    fs,
    sync::{broadcast, watch},
};
use tokio_stream::StreamExt;

// Config keys
const REMOTE_CONTROL_KEY: ConfigKey<SocketAddr> =
    ConfigKey::new("remote_control", "Remote control endpoint address");

// Repo metadata keys
const AUTOMOUNT_KEY: &str = "automount";
const DHT_ENABLED_KEY: &str = "dht_enabled";
const PEX_ENABLED_KEY: &str = "pex_enabled";

const REPOSITORY_FILE_EXTENSION: &str = "ouisyncdb";

pub(crate) struct State {
    pub config: ConfigStore,
    pub network: Network,
    pub tls_config: TlsConfig,
    store: Store,
    mounter: Mutex<Option<Arc<MultiRepoVFS>>>,
    repos: RepositorySet,
    files: FileSet,
    root_monitor: StateMonitor,
    repos_monitor: StateMonitor,
    remote_server: Mutex<Option<Arc<RemoteServer>>>,
    metrics_server: MetricsServer,
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

        let store = Store::new(store_dir);

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

        let tls_config = TlsConfig::new(config.dir().to_owned());

        let remote_server = match config.entry(REMOTE_CONTROL_KEY).get().await {
            Ok(addr) => Some(
                RemoteServer::bind(addr, tls_config.server().await?)
                    .await
                    .map_err(Error::Bind)?,
            ),
            Err(ConfigError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };

        let metrics_server = MetricsServer::init(&config, &network, &tls_config).await?;

        let state = Self {
            config,
            network,
            tls_config,
            store,
            mounter: Mutex::new(mounter.map(Arc::new)),
            root_monitor,
            repos_monitor,
            repos: RepositorySet::new(),
            files: FileSet::new(),
            remote_server: Mutex::new(remote_server.map(Arc::new)),
            metrics_server,
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

    pub async fn remote_accept(&self) -> io::Result<AcceptedRemoteConnection> {
        let server = self.remote_server.lock().unwrap().clone();

        if let Some(server) = server {
            server.accept().await
        } else {
            future::pending().await
        }
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

    /// Binds the network listeners to the specified interfaces.
    ///
    /// Up to four listeners can be bound, one for each combination of protocol (TCP or QUIC) and IP
    /// family (IPv4 or IPv6). The format of the interfaces is "PROTO/IP:PORT" where PROTO is "tcp"
    /// or "quic". If IP is IPv6, it needs to be enclosed in square brackets.
    ///
    /// If port is `0`, binds to a random port initially but on subsequent starts tries to use the
    /// same port (unless it's already taken). This can be useful to configuring port forwarding.
    #[api]
    pub async fn session_bind_network(&self, addrs: Vec<PeerAddr>) {
        self.config.entry(BIND_KEY).set(&addrs).await.ok();
        network::bind_with_reuse_ports(&self.network, &self.config, &addrs).await;
    }

    #[api]
    pub fn session_subscribe_to_network(&self) -> NetworkEventReceiver {
        self.network.subscribe()
    }

    /// Returns our Ouisync protocol version.
    ///
    /// In order to establish connections with peers, they must use the same protocol version as
    /// us.
    ///
    /// See also [Self::session_get_highest_seen_protocol_version]
    #[api]
    pub fn session_get_current_protocol_version(&self) -> u64 {
        self.network.current_protocol_version()
    }

    /// Returns the highest protocol version of all known peers.
    ///
    /// If this is higher than [our version](Self::session_get_current_protocol_version) it likely
    /// means we are using an outdated version of Ouisync. When a peer with higher protocol version
    /// is found, a [NetworkEvent::ProtocolVersionMismatch] is emitted.
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

    /// Returns the listener addresses of this Ouisync instance.
    #[api]
    pub fn session_get_local_listener_addrs(&self) -> Vec<PeerAddr> {
        self.network.listener_local_addrs()
    }

    /// Returns the listener addresses of the specified remote Ouisync instance. Works only if the
    /// remote control API is enabled on the remote instance. Typically used with cache servers.
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

    /// Returns info about all known peers (both discovered and explicitly added).
    ///
    /// When the set of known peers changes, a [NetworkEvent::PeerSetChange] is emitted. Calling
    /// this function afterwards returns the new peer info.
    #[api]
    pub fn session_get_peers(&self) -> Vec<PeerInfo> {
        self.network.peer_info_collector().collect()
    }

    /// Adds peers to connect to.
    ///
    /// Normally peers are discovered automatically (using Bittorrent DHT, Peer exchange or Local
    /// discovery) but this function is useful in case when the discovery is not available for any
    /// reason (e.g. in an isolated network).
    ///
    /// Note that peers added with this function are remembered across restarts. To forget peers,
    /// use [Self::session_remove_user_provided_peers].
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

    /// Removes peers previously added with [Self::session_add_user_provided_peers].
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

    /// Is local discovery enabled?
    #[api]
    pub fn session_is_local_discovery_enabled(&self) -> bool {
        self.network.is_local_discovery_enabled()
    }

    /// Enables/disables local discovery.
    #[api]
    pub async fn session_set_local_discovery_enabled(&self, enabled: bool) {
        self.config
            .entry(LOCAL_DISCOVERY_ENABLED_KEY)
            .set(&enabled)
            .await
            .ok();
        self.network.set_local_discovery_enabled(enabled);
    }

    /// Is port forwarding (UPnP) enabled?
    #[api]
    pub fn session_is_port_forwarding_enabled(&self) -> bool {
        self.network.is_port_forwarding_enabled()
    }

    /// Enables/disables port forwarding (UPnP).
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

    /// Returns the runtime id of this Ouisync instance.
    ///
    /// The runtime id is a unique identifier of this instance which is randomly generated every
    /// time Ouisync starts.
    #[api]
    pub fn session_get_runtime_id(&self) -> PublicRuntimeId {
        self.network.this_runtime_id()
    }

    #[api]
    pub fn session_get_network_stats(&self) -> Stats {
        self.network.stats()
    }

    #[api]
    pub async fn session_bind_remote_control(
        &self,
        addr: Option<SocketAddr>,
    ) -> Result<u16, Error> {
        let config_entry = self.config.entry(REMOTE_CONTROL_KEY);

        if let Some(addr) = addr {
            config_entry.set(&addr).await?;

            let tls_config = self.tls_config.server().await?;
            let remote_server = RemoteServer::bind(addr, tls_config)
                .await
                .map_err(Error::Bind)?;
            let port = remote_server.local_addr().port();

            *self.remote_server.lock().unwrap() = Some(Arc::new(remote_server));

            Ok(port)
        } else {
            config_entry.remove().await?;

            *self.remote_server.lock().unwrap() = None;

            Ok(0)
        }
    }

    #[api]
    pub fn session_get_remote_control_listener_addr(&self) -> Option<SocketAddr> {
        self.remote_server
            .lock()
            .unwrap()
            .as_ref()
            .map(|server| server.local_addr())
    }

    #[api]
    pub async fn session_bind_metrics(&self, addr: Option<SocketAddr>) -> Result<(), Error> {
        if let Some(addr) = addr {
            self.metrics_server
                .bind(&self.config, &self.network, &self.tls_config, addr)
                .await
        } else {
            self.metrics_server.unbind(&self.config).await
        }
    }

    #[api]
    pub fn session_get_metrics_listener_addr(&self) -> Option<SocketAddr> {
        todo!()
    }

    #[api]
    pub fn session_get_store_dir(&self) -> Option<PathBuf> {
        self.store_dir().map(|path| path.to_owned())
    }

    #[api]
    pub async fn session_set_store_dir(&self, path: PathBuf) -> Result<(), Error> {
        if !self.store.set(Some(path.clone())) {
            return Ok(());
        }

        self.config.entry(STORE_DIR_KEY).set(&path).await?;

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
    pub async fn session_set_mount_root(&self, path: Option<PathBuf>) -> Result<(), Error> {
        if path == self.mount_root() {
            return Ok(());
        }

        let config_entry = self.config.entry(MOUNT_DIR_KEY);

        if let Some(path) = path {
            config_entry.set(&path).await?;

            let mounter = MultiRepoVFS::create(path).await?;
            let mounter = Arc::new(mounter);
            let mounter = self.mounter.lock().unwrap().insert(mounter).clone();

            // Remount all mounted repos
            let repos: Vec<_> = self
                .repos
                .map(|_, holder| (holder.short_name().to_owned(), holder.repository().clone()));

            for (name, repo) in repos {
                if repo.metadata().get(AUTOMOUNT_KEY).await?.unwrap_or(false) {
                    mounter.insert(name, repo)?;
                }
            }
        } else {
            config_entry.remove().await?;
            *self.mounter.lock().unwrap() = None;
        }

        Ok(())
    }

    /// Checks whether the given string is a valid share token.
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

    /// Returns the access mode that the given token grants.
    #[api]
    pub fn session_get_share_token_access_mode(&self, token: ShareToken) -> AccessMode {
        token.access_mode()
    }

    /// Returns the suggested name for the repository corresponding to the given token.
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
        Ok(self.repos.find_by_subpath(&name)?)
    }

    #[api]
    pub fn session_list_repositories(&self) -> BTreeMap<PathBuf, RepositoryHandle> {
        self.repos
            .map(|handle, holder| (holder.path().to_owned(), handle))
    }

    /// Creates a new repository.
    ///
    /// - `path`: path to the repository file or name of the repository.
    /// - `read_secret`: local secret for reading the repository on this device only. Do not share
    ///   with peers!. If null, the repo won't be protected and anyone with physical access to the
    ///   device will be able to read it.
    /// - `write_secret`: local secret for writing to the repository on this device only. Do not
    ///   share with peers! Can be the same as `read_secret` if one wants to use only one secret
    ///   for both reading and writing.  Separate secrets are useful for plausible deniability. If
    ///   both `read_secret` and `write_secret` are `None`, the repo won't be protected and anyone
    ///   with physical access to the device will be able to read and write to it. If `read_secret`
    ///   is not `None` but `write_secret` is `None`, the repo won't be writable from this device.
    /// - `token`: used to share repositories between devices. If not `None`, this repo will be
    ///   linked with the repos with the same token on other devices. See also
    ///   [Self::repository_share]. This also determines the maximal access mode the repo can be
    ///   opened in. If `None`, it's *write* mode.
    #[api]
    #[expect(clippy::too_many_arguments)] // TODO: extract the args to a struct
    pub async fn session_create_repository(
        &self,
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
        let holder = RepositoryHolder::new(path, repo);

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
        repository::set_repository_expiration(holder.repository(), value).await?;

        tracing::info!(name = holder.short_name(), "repository created");

        // unwrap is ok because we already checked that the repo doesn't exist earlier and we have
        // exclusive access to this state.
        let handle = self.repos.try_insert(holder).unwrap();

        Ok(handle)
    }

    /// Delete a repository with the given name.
    #[api]
    pub async fn session_delete_repository_by_name(&self, name: String) -> Result<(), Error> {
        let handle = self.repos.find_by_subpath(&name)?;
        self.repository_delete(handle).await?;
        Ok(())
    }

    /// Opens an existing repository.
    ///
    /// - `path`: path to the local file the repo is stored in.
    /// - `local_secret`: a local secret. See the `read_secret` and `write_secret` params in
    ///   [Self::session_create_repository] for more details. If this repo uses local secret
    ///   (s), this determines the access mode the repo is opened in: `read_secret` opens it
    ///   in *read* mode, `write_secret` opens it in *write* mode and no secret or wrong secret
    ///   opens it in *blind* mode. If this repo doesn't use local secret(s), the repo is opened in
    ///   the maximal mode specified when the repo was created.
    #[api]
    pub async fn session_open_repository(
        &self,
        path: PathBuf,
        local_secret: Option<LocalSecret>,
    ) -> Result<RepositoryHandle, Error> {
        let path = self.store.normalize_repository_path(&path)?;
        let handle = if let Some((handle, repo)) =
            self.repos.find_by_path(&path).and_then(|handle| {
                self.repos
                    .with(handle, |holder| holder.repository().clone())
                    .map(|repo| (handle, repo))
            }) {
            // If `local_secret` provides higher access mode than what the repo currently has,
            // increase it. If not, the access mode remains unchanged.
            repo.set_access_mode(AccessMode::Write, local_secret)
                .await?;

            handle
        } else {
            let holder = self.load_repository(&path, local_secret, false).await?;
            // unwrap is ok because we already handled the case when the repo already exists.
            self.repos.try_insert(holder).unwrap()
        };

        Ok(handle)
    }

    /// Delete the repository
    #[api]
    pub async fn repository_delete(&self, repo: RepositoryHandle) -> Result<(), Error> {
        let Some(holder) = self.repos.remove(repo) else {
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

    /// Closes the repository.
    #[api]
    pub async fn repository_close(&self, repo: RepositoryHandle) -> Result<(), Error> {
        let holder = self.repos.remove(repo).ok_or(Error::InvalidArgument)?;

        if let Some(mounter) = self.mounter.lock().unwrap().as_ref() {
            if let Err(error) = mounter.remove(holder.short_name()) {
                tracing::error!(
                    ?error,
                    name = holder.short_name(),
                    "failed to unmount repository"
                );
            }
        }

        holder.close().await
    }

    /// Export repository to file
    #[api]
    pub async fn repository_export(
        &self,
        repo: RepositoryHandle,
        output_path: PathBuf,
    ) -> Result<PathBuf, Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;

        let output_path = if output_path.extension().is_some() {
            output_path
        } else {
            output_path.with_extension(REPOSITORY_FILE_EXTENSION)
        };

        repo.export(&output_path).await?;

        Ok(output_path)
    }

    /// Creates a *share token* to share this repository with other devices.
    ///
    /// By default the access mode of the token will be the same as the mode the repo is currently
    /// opened in but it can be escalated with the `local_secret` param or de-escalated with the
    /// `access_mode` param.
    ///
    /// - `access_mode`: access mode of the token. Useful to de-escalate the access mode to below of
    ///   what the repo is opened in.
    /// - `local_secret`: the local repo secret. If not `None`, the share token's access mode will
    ///   be the same as what the secret provides. Useful to escalate the access mode to above of
    ///   what the repo is opened in.
    #[api]
    pub async fn repository_share(
        &self,
        repo: RepositoryHandle,
        access_mode: AccessMode,
        local_secret: Option<LocalSecret>,
    ) -> Result<ShareToken, Error> {
        let (repo, short_name) = self
            .repos
            .get_repository_and_short_name(repo)
            .ok_or(Error::InvalidArgument)?;

        let access_secrets = if let Some(local_secret) = local_secret {
            repo.unlock_secrets(local_secret).await?
        } else {
            repo.secrets()
        };

        let share_token =
            ShareToken::from(access_secrets.with_mode(access_mode)).with_name(short_name);

        Ok(share_token)
    }

    #[api]
    pub fn repository_get_path(&self, repo: RepositoryHandle) -> Result<PathBuf, Error> {
        self.repos
            .with(repo, |holder| holder.path().to_owned())
            .ok_or(Error::InvalidArgument)
    }

    /// Return the info-hash of the repository formatted as hex string. This can be used as a
    /// globally unique, non-secret identifier of the repository.
    #[api]
    pub fn repository_get_info_hash(&self, repo: RepositoryHandle) -> Result<String, Error> {
        let info_hash = self
            .repos
            .with(repo, |holder| {
                ouisync::repository_info_hash(holder.repository().secrets().id())
            })
            .ok_or(Error::InvalidArgument)?;

        Ok(hex::encode(info_hash))
    }

    #[api]
    pub async fn repository_move(&self, repo: RepositoryHandle, dst: PathBuf) -> Result<(), Error> {
        move_repository::invoke(self, repo, &dst).await
    }

    #[api]
    pub async fn repository_reset_access(
        &self,
        repo: RepositoryHandle,
        token: ShareToken,
    ) -> Result<(), Error> {
        let new_credentials = Credentials::with_random_writer_id(token.into_secrets());
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        repo.set_credentials(new_credentials).await?;

        Ok(())
    }

    /// Sets, unsets or changes local secrets for accessing the repository or disables the given
    /// access mode.
    ///
    /// ## Examples
    ///
    /// To protect both read and write access with the same password:
    ///
    /// ```kotlin
    /// val password = Password("supersecret")
    /// repo.setAccess(read: AccessChange.Enable(password), write: AccessChange.Enable(password))
    /// ```
    ///
    /// To require password only for writing:
    ///
    /// ```kotlin
    /// repo.setAccess(read: AccessChange.Enable(null), write: AccessChange.Enable(password))
    /// ```
    ///
    /// To competelly disable write access but leave read access as it was. Warning: this operation
    /// is currently irreversibe.
    ///
    /// ```kotlin
    /// repo.setAccess(read: null, write: AccessChange.Disable)
    /// ```
    #[api]
    pub async fn repository_set_access(
        &self,
        repo: RepositoryHandle,
        read: Option<AccessChange>,
        write: Option<AccessChange>,
    ) -> Result<(), Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        repo.set_access(read, write).await?;
        Ok(())
    }

    /// Returns the access mode (*blind*, *read* or *write*) the repository is currently opened in.
    #[api]
    pub fn repository_get_access_mode(&self, repo: RepositoryHandle) -> Result<AccessMode, Error> {
        self.repos
            .with(repo, |holder| holder.repository().access_mode())
            .ok_or(Error::InvalidArgument)
    }

    /// Switches the repository to the given access mode.
    ///
    /// - `access_mode` is the desired access mode to switch to.
    /// - `local_secret` is the local secret protecting the desired access mode. Can be `None` if no
    ///   local secret is used.
    #[api]
    pub async fn repository_set_access_mode(
        &self,
        repo: RepositoryHandle,
        access_mode: AccessMode,
        local_secret: Option<LocalSecret>,
    ) -> Result<(), Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        repo.set_access_mode(access_mode, local_secret).await?;

        Ok(())
    }

    /// Gets the current credentials of this repository. Can be used to restore access after closing
    /// and reopening the repository.
    #[api]
    pub fn repository_get_credentials(&self, repo: RepositoryHandle) -> Result<Vec<u8>, Error> {
        self.repos
            .with(repo, |holder| holder.repository().credentials().encode())
            .ok_or(Error::InvalidArgument)
    }

    /// Sets the current credentials of the repository.
    #[api]
    pub async fn repository_set_credentials(
        &self,
        repo: RepositoryHandle,
        credentials: Vec<u8>,
    ) -> Result<(), Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        repo.set_credentials(Credentials::decode(&credentials)?)
            .await?;

        Ok(())
    }

    #[api]
    pub async fn repository_mount(&self, repo: RepositoryHandle) -> Result<PathBuf, Error> {
        let (repo, short_name) = self
            .repos
            .get_repository_and_short_name(repo)
            .ok_or(Error::InvalidArgument)?;

        repo.metadata().set(AUTOMOUNT_KEY, true).await?;

        let mounter = self.mounter.lock().unwrap();
        let Some(mounter) = mounter.as_ref() else {
            return Err(Error::OperationNotSupported);
        };

        if let Some(mount_point) = mounter.mount_point(&short_name) {
            // Already mounted
            Ok(mount_point)
        } else {
            Ok(mounter.insert(short_name, repo)?)
        }
    }

    #[api]
    pub async fn repository_unmount(&self, repo: RepositoryHandle) -> Result<(), Error> {
        let (repo, short_name) = self
            .repos
            .get_repository_and_short_name(repo)
            .ok_or(Error::InvalidArgument)?;

        repo.metadata().remove(AUTOMOUNT_KEY).await?;

        if let Some(mounter) = self.mounter.lock().unwrap().as_ref() {
            mounter.remove(&short_name)?;
        }

        Ok(())
    }

    #[api]
    pub fn repository_get_mount_point(
        &self,
        repo: RepositoryHandle,
    ) -> Result<Option<PathBuf>, Error> {
        let short_name = self
            .repos
            .with(repo, |holder| holder.short_name().to_owned())
            .ok_or(Error::InvalidArgument)?;

        Ok(self
            .mounter
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.mount_point(&short_name)))
    }

    #[api]
    pub fn repository_subscribe(
        &self,
        repo: RepositoryHandle,
    ) -> Result<RepositorySubscription, Error> {
        self.repos
            .with(repo, |holder| holder.repository().subscribe())
            .ok_or(Error::InvalidArgument)
    }

    /// Returns whether syncing with other replicas is enabled for this repository.
    #[api]
    pub fn repository_is_sync_enabled(&self, repo: RepositoryHandle) -> Result<bool, Error> {
        self.repos
            .with(repo, |holder| holder.is_sync_enabled())
            .ok_or(Error::InvalidArgument)
    }

    /// Enabled or disables syncing with other replicas.
    ///
    /// Note syncing is initially disabled.
    #[api]
    pub async fn repository_set_sync_enabled(
        &self,
        repo: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        let handle = repo;
        let (repo, was_enabled) = self
            .repos
            .with(handle, |holder| {
                (holder.repository().clone(), holder.is_sync_enabled())
            })
            .ok_or(Error::InvalidArgument)?;

        match (was_enabled, enabled) {
            (true, false) => {
                self.repos.with(handle, |holder| holder.disable_sync());
            }
            (false, true) => {
                let reg = register(&self.network, &repo).await?;
                self.repos.with(handle, |holder| holder.enable_sync(reg));
            }
            (true, true) | (false, false) => (),
        }

        Ok(())
    }

    /// Returns the synchronization progress of this repository as the number of bytes already
    /// synced ([Progress.value]) vs. the total size of the repository in bytes ([Progress.total]).
    #[api]
    pub async fn repository_get_sync_progress(
        &self,
        repo: RepositoryHandle,
    ) -> Result<Progress, Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;

        Ok(repo.sync_progress().await?)
    }

    /// Is Bittorrent DHT enabled?
    #[api]
    pub async fn repository_is_dht_enabled(&self, repo: RepositoryHandle) -> Result<bool, Error> {
        let result = self
            .repos
            .with(repo, |holder| {
                holder
                    .with_registration(|reg| reg.is_dht_enabled())
                    .ok_or_else(|| holder.repository().clone())
            })
            .ok_or(Error::InvalidArgument)?;

        match result {
            Ok(enabled) => Ok(enabled),
            Err(repo) => Ok(repo.metadata().get(DHT_ENABLED_KEY).await?.unwrap_or(false)),
        }
    }

    /// Enables/disabled Bittorrent DHT (for peer discovery).
    #[api]
    pub async fn repository_set_dht_enabled(
        &self,
        repo: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        let repo = self
            .repos
            .with(repo, |holder| {
                holder.with_registration(|reg| reg.set_dht_enabled(enabled));
                holder.repository().clone()
            })
            .ok_or(Error::InvalidArgument)?;

        set_metadata_bool(&repo, DHT_ENABLED_KEY, enabled).await?;

        Ok(())
    }

    /// Is Peer Exchange enabled?
    #[api]
    pub async fn repository_is_pex_enabled(&self, repo: RepositoryHandle) -> Result<bool, Error> {
        let result = self
            .repos
            .with(repo, |holder| {
                holder
                    .with_registration(|reg| reg.is_pex_enabled())
                    .ok_or_else(|| holder.repository().clone())
            })
            .ok_or(Error::InvalidArgument)?;

        match result {
            Ok(enabled) => Ok(enabled),
            Err(repo) => Ok(repo.metadata().get(PEX_ENABLED_KEY).await?.unwrap_or(false)),
        }
    }

    /// Enables/disables Peer Exchange (for peer discovery).
    #[api]
    pub async fn repository_set_pex_enabled(
        &self,
        repo: RepositoryHandle,
        enabled: bool,
    ) -> Result<(), Error> {
        let repo = self
            .repos
            .with(repo, |holder| {
                holder.with_registration(|reg| reg.set_pex_enabled(enabled));
                holder.repository().clone()
            })
            .ok_or(Error::InvalidArgument)?;

        set_metadata_bool(&repo, PEX_ENABLED_KEY, enabled).await?;

        Ok(())
    }

    #[api]
    pub fn repository_get_stats(&self, repo: RepositoryHandle) -> Result<Stats, Error> {
        self.repos
            .with(repo, |holder| {
                holder
                    .with_registration(|reg| reg.stats())
                    .unwrap_or_default()
            })
            .ok_or(Error::InvalidArgument)
    }

    /// Creates mirror of this repository on the given cache server host.
    ///
    /// Cache servers relay traffic between Ouisync peers and also temporarily store data. They are
    /// useful when direct P2P connection fails (e.g. due to restrictive NAT) and also to allow
    /// syncing when the peers are not online at the same time (they still need to be online within
    /// ~24 hours of each other).
    ///
    /// Requires the repository to be opened in write mode.
    #[api]
    pub async fn repository_create_mirror(
        &self,
        repo: RepositoryHandle,
        host: String,
    ) -> Result<(), Error> {
        let secrets = self
            .repos
            .with(repo, |holder| {
                holder.repository().secrets().into_write_secrets()
            })
            .ok_or(Error::InvalidArgument)?
            .ok_or(Error::PermissionDenied)?;

        let mut client = self.connect_remote_client(&host).await?;

        let result = client.create_mirror(&secrets).await;
        client.close().await;
        result?;

        Ok(())
    }

    /// Deletes mirror of this repository from the given cache server host.
    ///
    /// Requires the repository to be opened in write mode.
    #[api]
    pub async fn repository_delete_mirror(
        &self,
        repo: RepositoryHandle,
        host: String,
    ) -> Result<(), Error> {
        let secrets = self
            .repos
            .with(repo, |holder| {
                holder.repository().secrets().into_write_secrets()
            })
            .ok_or(Error::InvalidArgument)?
            .ok_or(Error::PermissionDenied)?;

        let mut client = self.connect_remote_client(&host).await?;
        let result = client.delete_mirror(&secrets).await;
        client.close().await;

        result?;

        Ok(())
    }

    /// Checks if this repository is mirrored on the given cache server host.
    #[api]
    pub async fn repository_mirror_exists(
        &self,
        repo: RepositoryHandle,
        host: String,
    ) -> Result<bool, Error> {
        let id = self
            .repos
            .with(repo, |holder| *holder.repository().secrets().id())
            .ok_or(Error::InvalidArgument)?;

        let mut client = self.connect_remote_client(&host).await?;
        let result = client.mirror_exists(&id).await;
        client.close().await;

        Ok(result?)
    }

    #[api]
    pub async fn repository_get_quota(&self, repo: RepositoryHandle) -> Result<QuotaInfo, Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;

        let quota = repo.quota().await?;
        let size = repo.size().await?;

        Ok(QuotaInfo { quota, size })
    }

    #[api]
    pub async fn repository_set_quota(
        &self,
        repo: RepositoryHandle,
        value: Option<StorageSize>,
    ) -> Result<(), Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        repo.set_quota(value).await?;

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
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        repository::set_repository_expiration(&repo, value).await
    }

    #[api]
    pub async fn repository_get_expiration(
        &self,
        repo: RepositoryHandle,
    ) -> Result<Option<Duration>, Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        repository::repository_expiration(&repo).await
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
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?
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
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?
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
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?
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
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;

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
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?
            .lookup_type(path)
            .await
        {
            Ok(entry_type) => Ok(Some(entry_type)),
            Err(ouisync::Error::EntryNotFound) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Moves an entry (file or directory) from `src` to `dst`.
    #[api]
    pub async fn repository_move_entry(
        &self,
        repo: RepositoryHandle,
        src: String,
        dst: String,
    ) -> Result<(), Error> {
        self.repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?
            .move_entry(src, dst)
            .await?;

        Ok(())
    }

    /// Creates a new directory at the given path in the repository.
    #[api]
    pub async fn repository_create_directory(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<(), Error> {
        self.repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?
            .create_directory(path)
            .await?;

        Ok(())
    }

    /// Returns the entries of the directory at the given path in the repository.
    #[api]
    pub async fn repository_read_directory(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<Vec<DirectoryEntry>, Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;

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
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;

        if recursive {
            repo.remove_entry_recursively(path).await?
        } else {
            repo.remove_entry(path).await?
        }

        Ok(())
    }

    /// Creates a new file at the given path in the repository.
    #[api]
    pub async fn repository_create_file(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<FileHandle, Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        let local_branch = repo.local_branch()?;

        let file = repo.create_file(&path).await?;
        let holder = FileHolder { file, local_branch };
        let handle = self.files.insert(holder);

        Ok(handle)
    }

    /// Opens an existing file at the given path in the repository.
    #[api]
    pub async fn repository_open_file(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<FileHandle, Error> {
        let repo = self
            .repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;
        let local_branch = repo.local_branch()?;

        let file = repo.open_file(path).await?;
        let holder = FileHolder { file, local_branch };
        let handle = self.files.insert(holder);

        Ok(handle)
    }

    /// Removes (deletes) the file at the given path from the repository.
    #[api]
    pub async fn repository_remove_file(
        &self,
        repo: RepositoryHandle,
        path: String,
    ) -> Result<(), Error> {
        self.repos
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?
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
            .get_repository(repo)
            .ok_or(Error::InvalidArgument)?;

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
        &self,
        file: FileHandle,
        offset: u64,
        size: u64,
    ) -> Result<Vec<u8>, Error> {
        let size = size as usize;
        let mut buffer = vec![0; size];

        let mut holder = self.files.get(file)?;

        holder.file.seek(SeekFrom::Start(offset));

        // TODO: consider using just `read`
        let size = holder.file.read_all(&mut buffer).await?;
        buffer.truncate(size);

        Ok(buffer)
    }

    /// Writes the data to the file at the given offset.
    #[api]
    pub async fn file_write(
        &self,
        file: FileHandle,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        let mut holder = self.files.get(file)?;

        let local_branch = holder.local_branch.clone();
        holder.file.seek(SeekFrom::Start(offset));
        holder.file.fork(local_branch).await?;

        // TODO: consider using just `write` and returning the number of bytes written
        holder.file.write_all(&data).await?;

        Ok(())
    }

    /// Returns the length of the file in bytes
    #[api]
    pub fn file_get_length(&self, file: FileHandle) -> Result<u64, Error> {
        Ok(self.files.get(file)?.file.len())
    }

    /// Returns the sync progress of this file, that is, the total byte size of all the blocks of
    /// this file that's already been downloaded.
    ///
    /// Note that Ouisync downloads the blocks in random order, so until the file's been completely
    /// downloaded, the already downloaded blocks are not guaranteed to continuous (there might be
    /// gaps).
    #[api]
    pub async fn file_get_progress(&self, file: FileHandle) -> Result<u64, Error> {
        // Don't keep the file locked while progress is being awaited.
        let progress = self.files.get(file)?.file.progress();
        let progress = progress.await?;

        Ok(progress)
    }

    /// Truncates the file to the given length.
    #[api]
    pub async fn file_truncate(&self, file: FileHandle, len: u64) -> Result<(), Error> {
        let mut holder = self.files.get(file)?;
        let local_branch = holder.local_branch.clone();
        holder.file.fork(local_branch).await?;
        holder.file.truncate(len)?;

        Ok(())
    }

    /// Flushes any pending writes to the file.
    #[api]
    pub async fn file_flush(&self, file: FileHandle) -> Result<(), Error> {
        self.files.get(file)?.file.flush().await?;

        Ok(())
    }

    /// Closes the file.
    #[api]
    pub async fn file_close(&self, file: FileHandle) -> Result<(), Error> {
        self.files.remove(file)?.file.flush().await?;

        Ok(())
    }

    #[api]
    pub fn session_generate_password_salt(&self) -> PasswordSalt {
        OsRng.r#gen()
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

    #[api]
    pub async fn session_subscribe_to_state_monitor(
        &self,
        path: Vec<MonitorId>,
    ) -> Result<StateMonitorSubscription, Error> {
        Ok(self
            .root_monitor
            .locate(path)
            .ok_or(Error::NotFound)?
            .subscribe())
    }

    #[api]
    pub async fn session_unsubscribe(&self, id: MessageId) -> Unsubscribe {
        Unsubscribe(id)
    }

    pub async fn set_all_repositories_sync_enabled(&self, enabled: bool) -> Result<(), Error> {
        let handles: Vec<_> = self.repos.map(|handle, _| handle);
        for handle in handles {
            self.repository_set_sync_enabled(handle, enabled).await?;
        }

        Ok(())
    }

    pub fn store_dir(&self) -> Option<PathBuf> {
        self.store.get()
    }

    pub fn mount_root(&self) -> Option<PathBuf> {
        self.mounter
            .lock()
            .unwrap()
            .as_ref()
            .map(|m| m.mount_root().to_owned())
    }

    pub async fn delete_expired_repositories(&self) {
        let repos: Vec<_> = self
            .repos
            .map(|handle, holder| (handle, holder.repository().clone()));
        let mut expired = Vec::new();

        for (handle, repo) in repos {
            let Some(last_block_expiration_time) = repo.last_block_expiration_time() else {
                continue;
            };

            let expiration = match repository::repository_expiration(&repo).await {
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

    /// Closes all resources
    pub async fn close(&self) {
        self.metrics_server.close();
        self.network.shutdown().await;
        self.close_repositories().await;
    }

    // Find all repositories in the store dir and open them.
    async fn load_repositories(&self) {
        let Some(store_dir) = self.store.get() else {
            tracing::warn!("store dir not specified");
            return;
        };

        if !fs::try_exists(&store_dir).await.unwrap_or(false) {
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

    async fn close_repositories(&self) {
        let repos: Vec<_> = self.repos.drain();

        // Unmount all repos
        if let Some(mounter) = self.mounter.lock().unwrap().as_ref() {
            for holder in &repos {
                if let Err(error) = mounter.remove(holder.short_name()) {
                    tracing::warn!(
                        ?error,
                        repo = holder.short_name(),
                        "failed to unmount repository",
                    );
                }
            }
        }

        for holder in repos {
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
        let mounter = self.mounter.lock().unwrap().as_ref().cloned();

        load_repository(
            path,
            local_secret,
            sync_enabled,
            &self.config,
            &self.network,
            &self.repos_monitor,
            mounter.as_deref(),
        )
        .await
    }

    async fn connect_remote_client(&self, host: &str) -> Result<RemoteClient, Error> {
        Ok(RemoteClient::connect(host, self.tls_config.client().await?).await?)
    }
}

// These definitions are only needed to make the api parser's job easier:
pub(crate) struct Unsubscribe(pub MessageId);
pub(crate) type RepositorySubscription = broadcast::Receiver<Event>;
pub(crate) type StateMonitorSubscription = watch::Receiver<()>;

struct Store {
    dir: Mutex<Option<PathBuf>>,
}

impl Store {
    fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir: Mutex::new(dir),
        }
    }

    fn get(&self) -> Option<PathBuf> {
        self.dir.lock().unwrap().as_ref().cloned()
    }

    fn set(&self, dir: Option<PathBuf>) -> bool {
        let mut guard = self.dir.lock().unwrap();

        if *guard != dir {
            *guard = dir;
            true
        } else {
            false
        }
    }

    fn normalize_repository_path(&self, path: &Path) -> Result<PathBuf, Error> {
        let dir = self.get();

        let path = if path.is_absolute() {
            Cow::Borrowed(path)
        } else {
            Cow::Owned(dir.as_deref().ok_or(Error::StoreDirUnspecified)?.join(path))
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
        let Some(store_dir) = self.get() else {
            return Ok(());
        };

        if !path.starts_with(&store_dir) {
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
    let holder = RepositoryHolder::new(path.to_owned(), repo);

    if sync_enabled {
        holder.enable_sync(register(network, holder.repository()).await?);
    }

    if let Some(mounter) = mounter {
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

async fn register(network: &Network, repo: &Repository) -> Result<Registration, Error> {
    let registration = network.register(repo.handle());

    let metadata = repo.metadata();
    registration.set_dht_enabled(metadata.get(DHT_ENABLED_KEY).await?.unwrap_or(false));
    registration.set_pex_enabled(metadata.get(PEX_ENABLED_KEY).await?.unwrap_or(false));

    Ok(registration)
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

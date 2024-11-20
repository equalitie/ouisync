use crate::repository::RepositoryHandle;
use ouisync::{crypto::Password, AccessMode, PeerAddr, SetLocalSecret, ShareToken, StorageSize};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf, str::FromStr};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
#[expect(clippy::large_enum_variant)]
pub enum Request {
    /// Enable/disable remote control endpoint
    RemoteControlBind { addrs: Vec<SocketAddr> },
    /// Enable/disable metrics collection endpoint
    MetricsBind { addr: Option<SocketAddr> },
    /// Find repository by name. Returns the repository that matches the name exactly or
    /// unambiguously by prefix.
    RepositoryFind(String),
    RepositoryCreate {
        name: String,
        read_secret: Option<SetLocalSecret>,
        write_secret: Option<SetLocalSecret>,
        share_token: Option<ShareToken>,
    },
    /// Delete a repository
    RepositoryDelete(RepositoryHandle),
    /// Export repository to a file
    RepositoryExport {
        handle: RepositoryHandle,
        output: PathBuf,
    },
    /// Import a repository from a file
    RepositoryImport {
        input: PathBuf,
        name: Option<String>,
        mode: ImportMode,
        force: bool,
    },
    /*
    Open {
        name: String,
        password: Option<String>,
    },
    Close {
        name: String,
    },
    /// Export a repository to a file
    ///
    /// Note currently this strips write access and removes local password (if any) from the
    /// exported repository. So if the repository is currently opened in write mode or read mode,
    /// it's exported in read mode. If it's in blind mode it's also exported in blind mode. This
    /// limitation might be lifted in the future.
    Export {
        /// Name of the repository to export
        name: String,

        /// File to export the repository to
        output: PathBuf,
    },
    /// Import a repository from a file
    Import {
        /// Name for the repository. Default is the filename the repository is imported from.
        name: Option<String>,

        /// How to import the repository
        mode: ImportMode,

        /// Overwrite the destination if it exists
        force: bool,

        /// File to import the repository from
        input: PathBuf,
    },
    /// Print share token for a repository
    Share {
        /// Name of the repository to share
        name: String,

        /// Access mode of the token ("blind", "read" or "write")
        mode: AccessMode,

        /// Local password
        password: Option<String>,
    },
    /// Mount repository
    Mount {
        /// Name of the repository to mount
        name: Option<String>,

        /// Mount all open and currently unmounted repositories
        all: bool,

        /// Path to mount the repository at
        path: Option<PathBuf>,
    },
    /// Unmount repository
    Unmount {
        /// Name of the repository to unmount
        name: Option<String>,

        /// Unmount all currently mounted repositories
        all: bool,
    },
    /// Mirror repository
    Mirror {
        /// Name of the repository to mirror
        name: String,

        /// Domain name or network address of the server to host the mirror
        host: String,
    },
    /// List open repositories
    ListRepositories,
    /// Bind the sync protocol to the specified addresses
    Bind {
        /// Addresses to bind to. PROTO is one of "quic" or "tcp", IP is a IPv4 or IPv6 address and
        /// PORT is a port number. If IP is 0.0.0.0 or [::] binds to all interfaces. If PORT is 0
        /// binds to a random port.
        ///
        /// Examples: quic/0.0.0.0:0, quic/[::]:0, tcp/192.168.0.100:55555
        addrs: Vec<PeerAddr>,
    },
    /// List addresses and ports we are listening on
    ListBinds,
    /// Enable or disable local discovery
    LocalDiscovery {
        /// Whether to enable or disable. If omitted, prints the current state.
        enabled: Option<bool>,
    },
    /// Enable or disable port forwarding
    PortForwarding {
        /// Whether to enable or disable. If omitted, prints the current state.
        enabled: Option<bool>,
    },
    /// Manually add peers.
    AddPeers {
        addrs: Vec<PeerAddr>,
    },
    /// Remove manually added peers.
    RemovePeers {
        addrs: Vec<PeerAddr>,
    },
    /// List all known peers.
    ///
    /// Prints one peer per line, each line consists of the following space-separated fields: ip,
    /// port, protocol, source, state, runtime id, active since, bytes sent, bytes received, last
    /// received at.
    ListPeers,
    /// Enable or disable DHT
    Dht {
        name: String,

        /// Whether to enable or disable. If omitted, prints the current state.
        enabled: Option<bool>,
    },
    /// Configure Peer Exchange (PEX)
    Pex {
        /// Name of the repository to enable/disable PEX for.
        name: Option<String>,

        /// Globally enable/disable sending contacts over PEX. If all of name, send, recv are
        /// omitted, prints the current state.
        send: Option<bool>,

        /// Globally enable/disable receiving contacts over PEX. If all of name, send, recv are
        /// omitted, prints the current state.
        recv: Option<bool>,

        /// Enable/disable PEX for the specified repository. If omitted, prints the current state.
        enabled: Option<bool>,
    },
    /// Get or set storage quota
    Quota {
        /// Name of the repository to get/set the quota for
        name: Option<String>,

        /// Get/set the default quota
        default: bool,

        /// Remove the quota
        remove: bool,

        /// Quota to set, in bytes. If omitted, prints the current quota. Support binary (ki, Mi,
        /// Ti, Gi, ...) and decimal (k, M, T, G, ...) suffixes.
        value: Option<StorageSize>,
    },
    /// Get or set block and repository expiration
    Expiration {
        /// Name of the repository to get/set the expiration for
        name: Option<String>,

        /// Get/set the default expiration
        default: bool,

        /// Remove the expiration
        remove: bool,

        /// Time after which blocks expire if not accessed (in seconds)
        block_expiration: Option<u64>,

        /// Time after which the whole repository is deleted after all its blocks expired
        /// (in seconds)
        repository_expiration: Option<u64>,
    },
    /// Set access to a repository corresponding to the share token
    SetAccess {
        /// Name of the repository which access shall be changed
        name: String,

        /// Repository token
        token: String,
    },
    /// Bind the remote API to the specified addresses.
    ///
    /// Overwrites any previously specified addresses.
    RemoteControl {
        /// Addresses to bind to. IP is a IPv4 or IPv6 address and PORT is a port number. If IP is
        /// 0.0.0.0 or [::] binds to all interfaces. If PORT is 0 binds to a random port. If empty
        /// disables the remote API.
        addrs: Vec<SocketAddr>,
    },
    /// Bind the metrics endpoint to the specified address.
    Metrics {
        /// Address to bind the metrics endpoint to. If specified, metrics collection is enabled
        /// and the collected metrics are served from this endpoint. If not specified, metrics
        /// collection is disabled.
        addr: Option<SocketAddr>,
    },
    */
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ImportMode {
    Copy,
    Move,
    SoftLink,
    HardLink,
}

#[derive(Error, Debug)]
#[error("invalid import mode")]
pub struct InvalidImportMode;

/*

    From ffi:

#[derive(Eq, PartialEq, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    RepositoryOpen {
        path: Utf8PathBuf,
        secret: Option<LocalSecret>,
    },
    RepositoryClose(RepositoryHandle),
    RepositorySubscribe(RepositoryHandle),
    ListRepositories,
    ListRepositoriesSubscribe,
    RepositoryIsSyncEnabled(RepositoryHandle),
    RepositorySetSyncEnabled {
        repository: RepositoryHandle,
        enabled: bool,
    },
    RepositoryRequiresLocalSecretForReading(RepositoryHandle),
    RepositoryRequiresLocalSecretForWriting(RepositoryHandle),
    RepositorySetAccess {
        repository: RepositoryHandle,
        read: Option<AccessChange>,
        write: Option<AccessChange>,
    },
    RepositoryCredentials(RepositoryHandle),
    RepositorySetCredentials {
        repository: RepositoryHandle,
        credentials: Bytes,
    },
    RepositoryAccessMode(RepositoryHandle),
    RepositorySetAccessMode {
        repository: RepositoryHandle,
        access_mode: AccessMode,
        secret: Option<LocalSecret>,
    },
    RepositoryName(RepositoryHandle),
    RepositoryInfoHash(RepositoryHandle),
    RepositoryDatabaseId(RepositoryHandle),
    RepositoryEntryType {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    RepositoryEntryVersionHash {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    RepositoryMoveEntry {
        repository: RepositoryHandle,
        src: Utf8PathBuf,
        dst: Utf8PathBuf,
    },
    RepositoryIsDhtEnabled(RepositoryHandle),
    RepositorySetDhtEnabled {
        repository: RepositoryHandle,
        enabled: bool,
    },
    RepositoryIsPexEnabled(RepositoryHandle),
    RepositorySetPexEnabled {
        repository: RepositoryHandle,
        enabled: bool,
    },
    RepositoryCreateShareToken {
        repository: RepositoryHandle,
        secret: Option<LocalSecret>,
        access_mode: AccessMode,
        name: Option<String>,
    },
    RepositorySyncProgress(RepositoryHandle),
    RepositoryCreateMirror {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryDeleteMirror {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryMirrorExists {
        repository: RepositoryHandle,
        host: String,
    },
    RepositoryMountAll(PathBuf),
    RepositoryGetMetadata {
        repository: RepositoryHandle,
        key: String,
    },
    RepositorySetMetadata {
        repository: RepositoryHandle,
        edits: Vec<MetadataEdit>,
    },
    RepositoryMount(RepositoryHandle),
    RepositoryUnmount(RepositoryHandle),
    RepositoryStats(RepositoryHandle),
    ShareTokenMode(#[serde(with = "as_str")] ShareToken),
    ShareTokenInfoHash(#[serde(with = "as_str")] ShareToken),
    ShareTokenSuggestedName(#[serde(with = "as_str")] ShareToken),
    ShareTokenNormalize(#[serde(with = "as_str")] ShareToken),
    ShareTokenMirrorExists {
        #[serde(with = "as_str")]
        share_token: ShareToken,
        host: String,
    },
    DirectoryCreate {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    DirectoryOpen {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    DirectoryExists {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    DirectoryRemove {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
        recursive: bool,
    },
    FileOpen {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileExists {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileCreate {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileRemove {
        repository: RepositoryHandle,
        path: Utf8PathBuf,
    },
    FileRead {
        file: FileHandle,
        offset: u64,
        len: u64,
    },
    FileWrite {
        file: FileHandle,
        offset: u64,
        data: Bytes,
    },
    FileTruncate {
        file: FileHandle,
        len: u64,
    },
    FileLen(FileHandle),
    FileProgress(FileHandle),
    FileFlush(FileHandle),
    FileClose(FileHandle),
    NetworkInit(NetworkDefaults),
    NetworkSubscribe,
    NetworkBind {
        #[serde(with = "as_option_str", default)]
        quic_v4: Option<SocketAddrV4>,
        #[serde(with = "as_option_str", default)]
        quic_v6: Option<SocketAddrV6>,
        #[serde(with = "as_option_str", default)]
        tcp_v4: Option<SocketAddrV4>,
        #[serde(with = "as_option_str", default)]
        tcp_v6: Option<SocketAddrV6>,
    },
    NetworkTcpListenerLocalAddrV4,
    NetworkTcpListenerLocalAddrV6,
    NetworkQuicListenerLocalAddrV4,
    NetworkQuicListenerLocalAddrV6,
    NetworkAddUserProvidedPeer(#[serde(with = "as_str")] PeerAddr),
    NetworkRemoveUserProvidedPeer(#[serde(with = "as_str")] PeerAddr),
    NetworkUserProvidedPeers,
    NetworkKnownPeers,
    NetworkThisRuntimeId,
    NetworkCurrentProtocolVersion,
    NetworkHighestSeenProtocolVersion,
    NetworkIsPortForwardingEnabled,
    NetworkSetPortForwardingEnabled(bool),
    NetworkIsLocalDiscoveryEnabled,
    NetworkSetLocalDiscoveryEnabled(bool),
    NetworkExternalAddrV4,
    NetworkExternalAddrV6,
    NetworkNatBehavior,
    NetworkStats,
    NetworkShutdown,
    StateMonitorGet(Vec<MonitorId>),
    StateMonitorSubscribe(Vec<MonitorId>),
    Unsubscribe(TaskHandle),
    GenerateSaltForSecretKey,
    DeriveSecretKey {
        password: String,
        salt: PasswordSalt,
    },
    GetReadPasswordSalt(RepositoryHandle),
    GetWritePasswordSalt(RepositoryHandle),
}
*/

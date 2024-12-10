use crate::defaults;
use clap::{
    builder::{ArgPredicate, BoolishValueParser},
    Parser, Subcommand, ValueEnum,
};
use ouisync::{AccessMode, PeerAddr, StorageSize};
use ouisync_bridge::logger::{LogColor, LogFormat};
use ouisync_service::protocol::ImportMode;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser, Debug)]
#[command(name = "ouisync", version, about)]
pub(crate) struct Options {
    /// Local socket (unix domain socket or windows named pipe) to connect to (if client) or to
    /// bind to (if server)
    ///
    /// Can be also specified with env variable OUISYNC_SOCKET.
    #[arg(short, long, default_value_os_t = defaults::socket(), value_name = "PATH")]
    pub socket: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    #[command(flatten)]
    Client(ClientCommand),
    #[command(flatten)]
    Server(ServerCommand),
}

#[derive(Subcommand, Debug)]
pub(crate) enum ServerCommand {
    /// Start the server
    Start {
        /// Config directory
        ///
        /// Can be also specified with env variable OUISYNC_CONFIG_DIR.
        #[arg(long, default_value_os_t = defaults::config_dir(), value_name = "PATH")]
        config_dir: PathBuf,

        /// Log format ("human" or "json")
        #[arg(long, default_value_t = LogFormat::Human)]
        log_format: LogFormat,

        /// When to color log messages ("always", "never" or "auto")
        ///
        /// "auto" means colors are used when printing directly to a terminal but not when
        /// redirected to a file or a pipe.
        #[arg(long, default_value_t)]
        log_color: LogColor,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum ClientCommand {
    /// Manually add peers.
    AddPeers {
        #[arg(required = true, value_name = "PROTO/IP:PORT")]
        addrs: Vec<PeerAddr>,
    },
    /// Configure network listeners.
    Bind {
        /// Addresses to listen on. PROTO is one of "quic" or "tcp", IP is a IPv4 or IPv6 address
        /// and PORT is a port number. If IP is 0.0.0.0 or [::] binds to all interfaces. If PORT is
        /// 0, binds to a random port. If unspecified, prints the current listeners.
        ///
        /// Examples: quic/0.0.0.0:0, quic/[::]:0, tcp/192.168.0.100:55555
        #[arg(value_name = "PROTO/IP:PORT")]
        addrs: Vec<PeerAddr>,

        /// Disable all listeners.
        #[arg(short, long, conflicts_with = "addrs")]
        disable: bool,
    },
    /// Create a new repository
    #[command(visible_alias = "mk")]
    Create {
        /// Name of the repository
        #[arg(required_unless_present = "token")]
        name: Option<String>,

        /// Repository token
        #[arg(short, long)]
        token: Option<String>,

        /// Local read and write password
        #[arg(short = 'P', long, conflicts_with_all = ["read_password", "write_password"])]
        password: Option<String>,

        /// Local read password
        #[arg(long)]
        read_password: Option<String>,

        /// Local write password
        #[arg(long)]
        write_password: Option<String>,
    },
    /// Delete a repository
    #[command(visible_alias = "rm")]
    Delete {
        /// Name of the repository to delete
        name: String,
    },
    /// Enable or disable DHT
    Dht {
        // Name of the repository to enable/disable DHT for
        name: String,

        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Get or set block and repository expiration
    #[command(alias = "expiry")]
    Expiration {
        /// Name of the repository to get/set the expiration for. If not specified, get/set the
        /// default expiration.
        name: Option<String>,

        /// Remove both the block and repository expiration.
        #[arg(short = 'R', long, conflicts_with_all = ["block", "repository"])]
        remove: bool,

        /// Time after which blocks expire if not accessed (in seconds)
        #[arg(short, long)]
        block: Option<u64>,

        /// Time after which the whole repository is deleted after all its blocks expired
        /// (in seconds)
        #[arg(short, long)]
        repository: Option<u64>,
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
        #[arg(value_name = "PATH")]
        output: PathBuf,
    },
    /// Import a repository from a file
    Import {
        /// File to import the repository from
        #[arg(value_name = "PATH")]
        input: PathBuf,

        /// Name for the repository. Default is the filename the repository is imported from.
        name: Option<String>,

        /// How to import the repository ("copy", "move", "softlink" or "hardlink")
        #[arg(short, long, default_value_t = ImportMode::Copy)]
        mode: ImportMode,

        /// Overwrite the destination if it exists
        #[arg(short, long)]
        force: bool,
    },
    /// List all known peers.
    ///
    /// Prints one peer per line, each line consists of the following space-separated fields: ip,
    /// port, protocol, source, state, runtime id, active since, bytes sent, bytes received, last
    /// received at.
    ListPeers,
    /// List all repositories
    #[command(visible_alias = "ls", alias = "list-repos")]
    ListRepositories,
    /// Enable or disable local discovery
    LocalDiscovery {
        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Configure endpoint for metrics collection.
    Metrics {
        /// Address to bind the metrics endpoint to. If not specified, prints the current endpoint.
        #[arg(value_name = "IP:PORT")]
        addr: Option<SocketAddr>,

        /// Disable metrics collection.
        #[arg(short, long, conflicts_with = "addr")]
        disable: bool,
    },
    /// Configure repository mirror
    Mirror {
        command: MirrorCommand,

        /// Name of the repository to mirror
        name: String,

        /// Domain name or network address of the server hosting the repository mirror.
        host: String,
    },
    /// Mount repository
    Mount {
        /// Name of the repository to mount. If unspecified, mounts all repositories.
        name: Option<String>,
    },
    /// Get or set the mount directory
    MountDir { path: Option<PathBuf> },
    /// Configure Peer Exchange (PEX)
    Pex {
        /// Name of the repository to enable/disable PEX for.
        name: Option<String>,

        /// Enable/disable PEX for the specified repository. If omitted, prints the current state.
        #[arg(
            requires_if(ArgPredicate::IsPresent, "name"),
            value_parser = BoolishValueParser::new(),
        )]
        enabled: Option<bool>,

        /// Globally enable/disable sending contacts over PEX. If all of name, send, recv are
        /// omitted, prints the current state.
        #[arg(
            short,
            long,
            conflicts_with_all = ["name", "enabled"],
            value_parser = BoolishValueParser::new(),
            value_name = "ENABLED"
        )]
        send: Option<bool>,

        /// Globally enable/disable receiving contacts over PEX. If all of name, send, recv are
        /// omitted, prints the current state.
        #[arg(
            short,
            long,
            conflicts_with_all = ["name", "enabled"],
            value_parser = BoolishValueParser::new(),
            value_name = "ENABLED"
        )]
        recv: Option<bool>,
    },
    /// Enable or disable port forwarding
    #[command(visible_alias = "upnp")]
    PortForwarding {
        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Get or set storage quota
    Quota {
        /// Name of the repository to get/set the quota for. If not specified, get/set the default
        /// quota.
        name: Option<String>,

        /// Remove the quota
        #[arg(short = 'R', long, conflicts_with = "value")]
        remove: bool,

        /// Quota to set, in bytes. If not specified, prints the current quota. Support binary
        /// (ki, Mi, Ti, Gi, ...) and decimal (k, M, T, G, ...) suffixes.
        value: Option<StorageSize>,
    },
    /// Configure remote control.
    RemoteControl {
        /// Address to bind the remote control endpoint to. IP is a IPv4 or IPv6 address and PORT is
        /// a port number. If IP is 0.0.0.0 or [::] binds to all interfaces. If PORT is 0 binds to
        /// a random port. If unspecified, prints the current remote control endpoint.
        #[arg(value_name = "IP:PORT")]
        addr: Option<SocketAddr>,

        /// Disable remote control.
        #[arg(short, long, conflicts_with = "addr")]
        disable: bool,
    },
    /// Remove manually added peers.
    RemovePeers {
        #[arg(required = true, value_name = "PROTO/IP:PORT")]
        addrs: Vec<PeerAddr>,
    },
    /// Reset access to a repository.
    ResetAccess {
        /// Name of the repository whose access shall be changed
        name: String,

        /// Repository token
        #[arg(short, long)]
        token: String,
    },
    /// Print the share token of a repository
    Share {
        /// Name of the repository to share
        name: String,

        /// Access mode of the token ("blind", "read" or "write")
        #[arg(short, long, default_value_t = AccessMode::Write, value_name = "MODE")]
        mode: AccessMode,

        /// Local password
        #[arg(short = 'P', long)]
        password: Option<String>,
    },
    /// Get or set the store directory
    StoreDir { path: Option<PathBuf> },
    /// Unmount repository
    #[command(alias = "umount")]
    Unmount {
        /// Name of the repository to unmount. If unspecified, unmounts all repositories.
        name: Option<String>,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum MirrorCommand {
    /// Create repository mirror on the specified server.
    Create,
    /// Delete repository mirror from the specified server.
    Delete,
    /// Check whether the repository is mirrored on the specified server.
    Exists,
}

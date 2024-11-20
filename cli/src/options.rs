use clap::{
    builder::{ArgPredicate, BoolishValueParser},
    Parser, Subcommand,
};
use ouisync::{AccessMode, PeerAddr};
use ouisync_bridge::logger::{LogColor, LogFormat};
use ouisync_service::protocol::ImportMode;
use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Parser, Debug)]
#[command(name = "ouisync", version, about)]
pub(crate) struct Options {
    /// Local socket (unix domain socket or windows named pipe) to connect to (if client) or to
    /// bind to (if server)
    ///
    /// Can be also specified with env variable OUISYNC_SOCKET.
    #[arg(short, long, default_value_os_t = default_socket(), value_name = "PATH")]
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
        #[arg(long, default_value_os_t = default_config_dir(), value_name = "PATH")]
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
    /// Listen on the specified addresses. Replaces all current listeners. If no addresses are
    /// specified, stops all current listeners.
    Bind {
        /// Addresses to listen on. PROTO is one of "quic" or "tcp", IP is a IPv4 or IPv6 address
        /// and PORT is a port number. If IP is 0.0.0.0 or [::] binds to all interfaces. If PORT is
        /// 0, binds to a random port.
        ///
        /// Examples: quic/0.0.0.0:0, quic/[::]:0, tcp/192.168.0.100:55555
        #[arg(value_name = "PROTO/IP:PORT")]
        addrs: Vec<PeerAddr>,
    },
    /// Create a new repository
    #[command(visible_aliases = ["mk"])]
    Create {
        /// Name of the repository
        #[arg(short, long, required_unless_present = "share_token")]
        name: Option<String>,

        /// Share token
        #[arg(short, long)]
        share_token: Option<String>,

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
    /// List addresses and ports we are listening on
    ListBinds,
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
    /// Bind the metrics collection endpoint to the specified address.
    Metrics {
        /// Address to bind the metrics endpoint to. If specified, metrics collection is enabled
        /// and the collected metrics are served from this endpoint. If not specified, metrics
        /// collection is disabled.
        #[arg(value_name = "IP:PORT")]
        addr: Option<SocketAddr>,
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
    /// Bind the remote API to the specified addresses.
    ///
    /// Overwrites any previously specified addresses.
    RemoteControl {
        /// Addresses to bind to. IP is a IPv4 or IPv6 address and PORT is a port number. If IP is
        /// 0.0.0.0 or [::] binds to all interfaces. If PORT is 0 binds to a random port. If empty
        /// disables the remote API.
        #[arg(value_name = "IP:PORT")]
        addrs: Vec<SocketAddr>,
    },
    /// Remove manually added peers.
    RemovePeers {
        #[arg(required = true, value_name = "PROTO/IP:PORT")]
        addrs: Vec<PeerAddr>,
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
    /*
    /// Mirror repository
    Mirror {
        /// Name of the repository to mirror
        #[arg(short, long)]
        name: String,

        /// Domain name or network address of the server to host the mirror
        #[arg(short = 'H', long)]
        host: String,
    },
    /// Get or set storage quota
    Quota {
        /// Name of the repository to get/set the quota for
        #[arg(
            short,
            long,
            required_unless_present = "default",
            conflicts_with = "default"
        )]
        name: Option<String>,

        /// Get/set the default quota
        #[arg(short, long)]
        default: bool,

        /// Remove the quota
        #[arg(short, long, conflicts_with = "value")]
        remove: bool,

        /// Quota to set, in bytes. If omitted, prints the current quota. Support binary (ki, Mi,
        /// Ti, Gi, ...) and decimal (k, M, T, G, ...) suffixes.
        value: Option<StorageSize>,
    },
    /// Get or set block and repository expiration
    #[command(alias = "expiry")]
    Expiration {
        /// Name of the repository to get/set the expiration for
        #[arg(
            short,
            long,
            required_unless_present = "default",
            conflicts_with = "default"
        )]
        name: Option<String>,

        /// Get/set the default expiration
        #[arg(short, long)]
        default: bool,

        /// Remove the expiration
        #[arg(short, long, conflicts_with = "value")]
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
        #[arg(short, long)]
        name: String,

        /// Repository token
        #[arg(short, long)]
        token: String,
    },

    */
}

/// Path to the config directory.
fn default_config_dir() -> PathBuf {
    env::var_os("OUISYNC_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .expect("config dir not defined")
                .join("ouisync")
        })
}

fn default_socket() -> PathBuf {
    env::var_os("OUISYNC_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(platform::default_socket)
}

mod platform {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn default_socket() -> PathBuf {
        // FIXME: when running as root, we should use `/run`
        dirs::runtime_dir()
            .or_else(dirs::cache_dir)
            .expect("neither runtime dir nor cache dir defined")
            .join("ouisync")
            .with_extension("sock")
    }

    #[cfg(target_os = "windows")]
    pub(super) fn default_socket() -> PathBuf {
        format!(r"\\.\pipe\{APP_NAME}").into()
    }
}

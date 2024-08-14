use chrono::{DateTime, SecondsFormat, Utc};
use clap::{
    builder::{ArgPredicate, BoolishValueParser},
    Subcommand, ValueEnum,
};
use ouisync_lib::{AccessMode, PeerAddr, PeerInfo, PeerSource, PeerState, StorageSize};
use serde::{Deserialize, Serialize};
use std::{
    fmt, iter,
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, SystemTime},
};

#[derive(Subcommand, Debug, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    /// Start the server
    Start,
    /// Bind the remote API to the specified addresses.
    ///
    /// Overwrites any previously specified addresses.
    BindRpc {
        /// Addresses to bind to. IP is a IPv4 or IPv6 address and PORT is a port number. If IP is
        /// 0.0.0.0 or [::] binds to all interfaces. If PORT is 0 binds to a random port. If empty
        /// disables the remote API.
        #[arg(value_name = "IP:PORT")]
        addrs: Vec<SocketAddr>,
    },
    /// Bind the metrics endpoint to the specified address.
    BindMetrics {
        /// Address to bind the metrics endpoint to. If specified, metrics collection is enabled
        /// and the collected metrics are served from this endpoint. If not specified, metrics
        /// collection is disabled.
        #[arg(value_name = "IP:PORT")]
        addr: Option<SocketAddr>,
    },
    /// Create a new repository
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
        #[arg(short, long)]
        name: String,
    },
    /// Open a repository
    Open {
        #[arg(short, long)]
        name: String,

        /// Local password
        #[arg(short = 'P', long)]
        password: Option<String>,
    },
    /// Close a repository
    Close {
        #[arg(short, long)]
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
        #[arg(short, long)]
        name: String,

        /// File to export the repository to
        #[arg(value_name = "PATH")]
        output: PathBuf,
    },
    /// Import a repository from a file
    Import {
        /// Name for the repository. Default is the filename the repository is imported from.
        #[arg(short, long)]
        name: Option<String>,

        /// How to import the repository
        #[arg(short, long, value_enum, default_value_t = ImportMode::Copy)]
        mode: ImportMode,

        /// Overwrite the destination if it exists
        #[arg(short, long)]
        force: bool,

        /// File to import the repository from
        #[arg(value_name = "PATH")]
        input: PathBuf,
    },
    /// Print share token for a repository
    Share {
        /// Name of the repository to share
        #[arg(short, long)]
        name: String,

        /// Access mode of the token ("blind", "read" or "write")
        #[arg(short, long, default_value_t = AccessMode::Write, value_name = "MODE")]
        mode: AccessMode,

        /// Local password
        #[arg(short = 'P', long)]
        password: Option<String>,
    },
    /// Mount repository
    Mount {
        /// Name of the repository to mount
        #[arg(short, long, required_unless_present = "all", conflicts_with = "all")]
        name: Option<String>,

        /// Mount all open and currently unmounted repositories
        #[arg(short, long)]
        all: bool,

        /// Path to mount the repository at
        #[arg(short, long, conflicts_with = "all")]
        path: Option<PathBuf>,
    },
    /// Unmount repository
    #[command(alias = "umount")]
    Unmount {
        /// Name of the repository to unmount
        #[arg(short, long, required_unless_present = "all", conflicts_with = "all")]
        name: Option<String>,

        /// Unmount all currently mounted repositories
        #[arg(short, long)]
        all: bool,
    },
    /// Mirror repository
    Mirror {
        /// Name of the repository to mirror
        #[arg(short, long)]
        name: String,

        /// Domain name or network address of the server to host the mirror
        #[arg(short = 'H', long)]
        host: String,
    },
    /// List open repositories
    #[command(visible_alias = "ls", alias = "list-repos")]
    ListRepositories,
    /// Bind the sync protocol to the specified addresses
    Bind {
        /// Addresses to bind to. PROTO is one of "quic" or "tcp", IP is a IPv4 or IPv6 address and
        /// PORT is a port number. If IP is 0.0.0.0 or [::] binds to all interfaces. If PORT is 0
        /// binds to a random port.
        ///
        /// Examples: quic/0.0.0.0:0, quic/[::]:0, tcp/192.168.0.100:55555
        #[arg(value_name = "PROTO/IP:PORT")]
        addrs: Vec<PeerAddr>,
    },
    /// List addresses and ports we are listening on
    ListBinds,
    /// Enable or disable local discovery
    LocalDiscovery {
        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Enable or disable port forwarding
    #[command(visible_alias = "upnp")]
    PortForwarding {
        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Manually add peers.
    AddPeers {
        #[arg(required = true, value_name = "PROTO/IP:PORT")]
        addrs: Vec<PeerAddr>,
    },
    /// Remove manually added peers.
    RemovePeers {
        #[arg(required = true, value_name = "PROTO/IP:PORT")]
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
        #[arg(short = 'n', long)]
        name: String,

        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Configure Peer Exchange (PEX)
    Pex {
        /// Name of the repository to enable/disable PEX for.
        #[arg(short = 'n', long)]
        name: Option<String>,

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

        /// Enable/disable PEX for the specified repository. If omitted, prints the current state.
        #[arg(
            requires_if(ArgPredicate::IsPresent, "name"),
            value_parser = BoolishValueParser::new(),
        )]
        enabled: Option<bool>,
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
    /// Get or set block expiration
    BlockExpiration {
        /// Name of the repository to get/set the block expiration for
        #[arg(
            short,
            long,
            required_unless_present = "default",
            conflicts_with = "default"
        )]
        name: Option<String>,

        /// Get/set the default block expiration
        #[arg(short, long)]
        default: bool,

        /// Remove the block expiration
        #[arg(short, long, conflicts_with = "value")]
        remove: bool,

        /// Set duration after which blocks are removed if not used (in seconds).
        value: Option<u64>,
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
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, ValueEnum)]
pub(crate) enum ImportMode {
    Copy,
    Move,
    SoftLink,
    HardLink,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    None,
    Bool(bool),
    String(String),
    Strings(Vec<String>),
    PeerInfo(Vec<PeerInfo>),
    PeerAddrs(Vec<PeerAddr>),
    SocketAddrs(Vec<SocketAddr>),
    StorageSize(StorageSize),
    QuotaInfo(QuotaInfo),
    BlockExpiration(Option<Duration>),
}

impl From<()> for Response {
    fn from(_: ()) -> Self {
        Self::None
    }
}

impl From<bool> for Response {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<String> for Response {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl<'a> From<&'a str> for Response {
    fn from(value: &'a str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<Vec<String>> for Response {
    fn from(value: Vec<String>) -> Self {
        Self::Strings(value)
    }
}

impl From<Vec<PeerInfo>> for Response {
    fn from(value: Vec<PeerInfo>) -> Self {
        Self::PeerInfo(value)
    }
}

impl From<Vec<PeerAddr>> for Response {
    fn from(value: Vec<PeerAddr>) -> Self {
        Self::PeerAddrs(value)
    }
}

impl From<Vec<SocketAddr>> for Response {
    fn from(value: Vec<SocketAddr>) -> Self {
        Self::SocketAddrs(value)
    }
}

impl From<StorageSize> for Response {
    fn from(value: StorageSize) -> Self {
        Self::StorageSize(value)
    }
}

impl From<QuotaInfo> for Response {
    fn from(value: QuotaInfo) -> Self {
        Self::QuotaInfo(value)
    }
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Bool(value) => write!(f, "{value}"),
            Self::String(value) => write!(f, "{value}"),
            Self::Strings(value) => {
                for item in value {
                    writeln!(f, "{item}")?;
                }

                Ok(())
            }
            Self::PeerInfo(value) => {
                for peer in value {
                    writeln!(f, "{}", PeerInfoDisplay(peer))?;
                }

                Ok(())
            }
            Self::PeerAddrs(addrs) => {
                for addr in addrs {
                    writeln!(f, "{}", PeerAddrDisplay(addr))?;
                }

                Ok(())
            }
            Self::SocketAddrs(value) => {
                for addr in value {
                    writeln!(f, "{addr}")?;
                }

                Ok(())
            }
            Self::StorageSize(value) => write!(f, "{value}"),
            Self::QuotaInfo(info) => write!(f, "{info}"),
            Self::BlockExpiration(info) => write!(f, "{info:?}"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Error {
    message: String,
    sources: Vec<String>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            sources: Vec::new(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            write!(f, "Error: {}", self.message)?;

            if !self.sources.is_empty() {
                writeln!(f)?;
                writeln!(f)?;
                write!(f, "Caused by:")?;
            }

            for (index, source) in self.sources.iter().enumerate() {
                writeln!(f)?;
                write!(f, "{index:>4}: {source}")?;
            }

            Ok(())
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl<E> From<E> for Error
where
    E: std::error::Error,
{
    fn from(src: E) -> Self {
        let message = src.to_string();
        let sources = iter::successors(src.source(), |error| error.source())
            .map(|error| error.to_string())
            .collect();

        Self { message, sources }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct QuotaInfo {
    pub quota: Option<StorageSize>,
    pub size: StorageSize,
}

impl fmt::Display for QuotaInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "quota:     ")?;

        if let Some(quota) = self.quota {
            writeln!(f, "{quota}")?;
        } else {
            writeln!(f, "∞")?;
        }

        write!(f, "available: ")?;

        if let Some(quota) = self.quota {
            let available = quota.saturating_sub(self.size);

            writeln!(
                f,
                "{} ({:.0}%)",
                available,
                percent(available.to_bytes(), quota.to_bytes())
            )?;
        } else {
            writeln!(f, "∞")?;
        }

        write!(f, "used:      {}", self.size)?;

        if let Some(quota) = self.quota {
            writeln!(
                f,
                " ({:.0}%)",
                percent(self.size.to_bytes(), quota.to_bytes())
            )?;
        } else {
            writeln!(f)?;
        }

        Ok(())
    }
}

fn percent(num: u64, den: u64) -> f64 {
    if den > 0 {
        100.0 * num as f64 / den as f64
    } else {
        0.0
    }
}

struct PeerAddrDisplay<'a>(&'a PeerAddr);

impl fmt::Display for PeerAddrDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            self.0.ip(),
            self.0.port(),
            match self.0 {
                PeerAddr::Tcp(_) => "tcp",
                PeerAddr::Quic(_) => "quic",
            },
        )
    }
}

struct PeerInfoDisplay<'a>(&'a PeerInfo);

impl fmt::Display for PeerInfoDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            PeerAddrDisplay(&self.0.addr),
            match self.0.source {
                PeerSource::UserProvided => "user-provided",
                PeerSource::Listener => "listener",
                PeerSource::LocalDiscovery => "local-discovery",
                PeerSource::Dht => "dht",
                PeerSource::PeerExchange => "pex",
            },
            match self.0.state {
                PeerState::Known => "known",
                PeerState::Connecting => "connecting",
                PeerState::Handshaking => "handshaking",
                PeerState::Active { .. } => "active",
            },
        )?;

        if let PeerState::Active { id, since } = &self.0.state {
            write!(
                f,
                " {} {} {} {} {}",
                id.as_public_key(),
                format_time(*since),
                self.0.stats.send,
                self.0.stats.recv,
                format_time(self.0.stats.recv_at),
            )?;
        }

        Ok(())
    }
}

fn format_time(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync_lib::{PeerSource, PeerState, SecretRuntimeId, TrafficStats};
    use rand::{rngs::StdRng, SeedableRng};
    use std::net::Ipv4Addr;

    #[test]
    fn peer_info_display() {
        let mut rng = StdRng::seed_from_u64(0);

        let addr: SocketAddr = (Ipv4Addr::LOCALHOST, 1248).into();
        let runtime_id = SecretRuntimeId::generate(&mut rng).public();

        assert_eq!(
            PeerInfoDisplay(&PeerInfo {
                addr: PeerAddr::Quic(addr),
                source: PeerSource::Dht,
                state: PeerState::Connecting,
                stats: TrafficStats::default(),
            })
            .to_string(),
            "127.0.0.1 1248 quic dht connecting"
        );

        assert_eq!(
            PeerInfoDisplay(&PeerInfo {
                addr: PeerAddr::Quic(addr),
                source: PeerSource::Dht,
                state: PeerState::Active {
                    id: runtime_id,
                    since: DateTime::parse_from_rfc3339("2024-06-12T02:30:00Z")
                        .unwrap()
                        .into(),
                },
                stats: TrafficStats {
                    send: 1024,
                    recv: 4096,
                    recv_at: DateTime::parse_from_rfc3339("2024-06-12T14:00:00Z")
                        .unwrap()
                        .into(),
                },
            })
            .to_string(),
            "127.0.0.1 \
             1248 \
             quic \
             dht \
             active \
             ee1aa49a4459dfe813a3cf6eb882041230c7b2558469de81f87c9bf23bf10a03 \
             2024-06-12T02:30:00Z \
             1024 \
             4096 \
             2024-06-12T14:00:00Z"
        );
    }
}

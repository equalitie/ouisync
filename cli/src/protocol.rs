use clap::{builder::BoolishValueParser, Subcommand};
use ouisync_bridge::logger::LogFormat;
use ouisync_lib::{AccessMode, PeerAddr, PeerInfo, StorageSize};
use serde::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, path::PathBuf};

#[derive(Subcommand, Debug, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    /// Start the server
    Start {
        /// Log format ("human" or "json")
        #[arg(long, default_value_t = LogFormat::Human)]
        log_format: LogFormat,
    },
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
        #[arg(short, long, required_unless_present = "all", conflicts_with = "all")]
        name: Option<String>,

        /// Mount all open and currently unmounted repositories
        #[arg(short, long)]
        all: bool,

        #[arg(short, long, conflicts_with = "all")]
        path: Option<PathBuf>,
    },
    /// Unmount repository
    #[command(alias = "umount")]
    Unmount {
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
    /// List protocol ports we are listening on
    ListPorts,
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
    /// List all known peers
    ListPeers,
    /// Enable or disable DHT
    Dht {
        #[arg(short = 'n', long)]
        name: String,

        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Enable or disable Peer Exchange (PEX)
    Pex {
        #[arg(short = 'n', long)]
        name: String,

        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
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
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    None,
    Bool(bool),
    String(String),
    Strings(Vec<String>),
    PeerInfo(Vec<PeerInfo>),
    SocketAddrs(Vec<SocketAddr>),
    StorageSize(StorageSize),
    QuotaInfo(QuotaInfo),
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
                    writeln!(
                        f,
                        "{}:{} ({:?}, {:?})",
                        peer.ip, peer.port, peer.source, peer.state
                    )?;
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
        }
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

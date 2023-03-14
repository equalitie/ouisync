use crate::{host_addr::HostAddr, APP_NAME};
use camino::Utf8PathBuf;
use clap::{builder::BoolishValueParser, Args, Parser, Subcommand};
use ouisync_lib::{AccessMode, PeerAddr, PeerInfo};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Parser, Debug)]
#[command(name = APP_NAME, version, about)]
pub(crate) struct Options {
    #[command(flatten)]
    pub dirs: Dirs,

    /// Host socket to connect to
    #[arg(short = 'H', long, default_value_t, value_name = "ADDR")]
    pub host: HostAddr,

    #[command(subcommand)]
    pub request: Request,
}

#[derive(Args, Debug)]
pub(crate) struct Dirs {
    /// Config directory
    #[arg(long, default_value_t = default_config_dir(), value_name = "PATH")]
    pub config_dir: Utf8PathBuf,

    /// Repositories storage directory
    #[arg(long, default_value_t = default_data_dir(), value_name = "PATH")]
    pub store_dir: Utf8PathBuf,

    /// Mount directory
    #[arg(long, default_value_t = default_mount_dir(), value_name = "PATH")]
    pub mount_dir: Utf8PathBuf,
}

#[derive(Subcommand, Debug, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Request {
    /// Start a server
    Serve,
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
        path: Option<Utf8PathBuf>,
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
    /// Bind to the specified addresses
    Bind {
        /// Addresses of the form "PROTO/IP:PORT" where PROTO is one of "quic" or "tcp", IP is
        /// a IPv4 or IPv6 address and PORT is a port number.
        /// If IP is 0.0.0.0 or [::] binds to all interfaces. If PORT is 0 binds to a random port.
        /// Examples: quic/0.0.0.0:0, quic/[::]:0, tcp/192.168.0.100:55555
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
        /// Addresses of the form "PROTO/IP:PORT" where PROTO is one of "quic" or "tcp", IP is
        /// a IPv4 or IPv6 address and PORT is a port number.
        #[arg(required = true)]
        addrs: Vec<PeerAddr>,
    },
    /// Remove manually added peers.
    RemovePeers {
        /// Addresses of the form "PROTO/IP:PORT" where PROTO is one of "quic" or "tcp", IP is
        /// a IPv4 or IPv6 address and PORT is a port number.
        #[arg(required = true)]
        addrs: Vec<PeerAddr>,
    },
    /// List all known peers
    ListPeers,
    /// Enable or disable DHT
    Dht {
        #[arg(short = 'n', long)]
        repository_name: String,

        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
    /// Enable or disable Peer Exchange (PEX)
    Pex {
        #[arg(short = 'n', long)]
        repository_name: String,

        /// Whether to enable or disable. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    None,
    Bool(bool),
    String(String),
    Strings(Vec<String>),
    PeerInfo(Vec<PeerInfo>),
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

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "OK"),
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
        }
    }
}

/// Path to the config directory.
fn default_config_dir() -> Utf8PathBuf {
    dirs::config_dir()
        .expect("config dir not defined")
        .join(APP_NAME)
        .try_into()
        .expect("invalid utf8 path")
}

fn default_data_dir() -> Utf8PathBuf {
    dirs::data_dir()
        .expect("data dir not defined")
        .join(APP_NAME)
        .try_into()
        .expect("invalid utf8 path")
}

fn default_mount_dir() -> Utf8PathBuf {
    dirs::home_dir()
        .expect("home dir not defined")
        .join(APP_NAME)
        .try_into()
        .expect("invalid utf8 path")
}

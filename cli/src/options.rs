use crate::{host_addr::HostAddr, path::PathBuf, APP_NAME};
use clap::{builder::BoolishValueParser, Parser, Subcommand};
use ouisync_lib::{AccessMode, PeerAddr};

#[derive(Parser, Debug)]
#[command(name = APP_NAME, version, about)]
pub(crate) struct Options {
    /// Config directory
    #[arg(long, default_value_t = default_config_dir(), value_name = "PATH")]
    pub config_dir: PathBuf,

    /// Data directory
    #[arg(long, default_value_t = default_data_dir(), value_name = "PATH")]
    pub data_dir: PathBuf,

    /// Host socket to connect to
    #[arg(short = 'H', long, default_value_t, value_name = "ADDR")]
    pub host: HostAddr,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
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
    /// Bind to the specified addresses
    Bind {
        /// Addresses of the form "PROTO/IP:PORT" where PROTO is one of "quic" or "tcp", IP is
        /// a IPv4 or IPv6 address and PORT is a port number.
        /// If IP is 0.0.0.0 or [::] binds to all interfaces. If PORT is 0 binds to a random port.
        /// Examples: quic/0.0.0.0:0, quic/[::]:0, tcp/192.168.0.100:55555
        addrs: Vec<PeerAddr>,
    },
    #[command(visible_alias = "lpd")]
    /// Enable or disable local peer discovery
    LocalDiscovery {
        /// Whether to enable or disable local peer discovery. If omitted, prints the current state.
        #[arg(value_parser = BoolishValueParser::new())]
        enabled: Option<bool>,
    },
}

/// Path to the config directory.
fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .expect("config dir not defined")
        .join(APP_NAME)
        .into()
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("data dir not defined")
        .join(APP_NAME)
        .into()
}

/*
use crate::APP_NAME;
use ouisync_lib::{
    crypto::{cipher::SecretKey, Password},
    AccessMode, LocalSecret, PeerAddr, ShareToken,
};
use std::{
    path::{Path, PathBuf},
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
};

/// Command line options.
#[derive(Parser, Debug)]
pub(crate) struct Options {
    /// Addresses to bind to. The expected format is {tcp,quic}/IP:PORT. Note that there may be at
    /// most one of each (protoco x IP-version) combinations. If more are specified, only the first
    /// one is used.
    #[clap(long, default_values = &["quic/0.0.0.0:0", "quic/[::]:0"], value_name = "proto/ip:port")]
    pub bind: Vec<PeerAddr>,

    /// Disable UPnP
    #[clap(long)]
    pub disable_upnp: bool,

    /// Disable local discovery
    #[clap(short, long)]
    pub disable_local_discovery: bool,

    /// Disable DHT
    #[clap(long)]
    pub disable_dht: bool,

    /// Disable Peer Exchange
    #[clap(long)]
    pub disable_pex: bool,

    /// Explicit list of {tcp,quic}/IP:PORT addresses of peers to connect to
    #[clap(long)]
    pub peers: Vec<PeerAddr>,

    /// Mount the named repository at the specified path. Can be specified multiple times to mount
    /// multiple repositories.
    #[clap(short, long, value_name = "NAME:PATH")]
    pub mount: Vec<Named<PathBuf>>,

    /// Pre-hashed 32 byte long (64 hexadecimal characters) local secret per repository. This is
    /// mainly intended for testing as password derivation is intentionally slow and some of the
    /// tests may timeout if the `password` argument is used instead. For all other use cases,
    /// prefer to use the `password` argument instead.
    // TODO: Zeroize
    #[clap(long, value_name = "NAME:KEY")]
    pub key: Vec<Named<String>>,

    /// Prints the listening port to the stdout when the replica becomes ready.
    /// Note this flag is unstable and experimental.
    #[clap(long)]
    pub print_port: bool,

    /// Prints the device id to the stdout when the replica becomes ready.
    /// Note this flag is unstable and experimental.
    #[clap(long)]
    pub print_device_id: bool,
}

impl Options {
    pub fn secret_for_repo(&self, repo_name: &str) -> Result<Option<LocalSecret>> {
        let key = self
            .key
            .iter()
            .find(|e| e.name == repo_name)
            .map(|e| e.value.as_str())
            .map(|k| LocalSecret::SecretKey(SecretKey::parse_hex(k).expect("failed to parse key")));

        let pwd = self
            .password
            .iter()
            .find(|e| e.name == repo_name)
            .map(|e| e.value.as_str())
            .map(|k| LocalSecret::Password(Password::new(k)));

        match (key, pwd) {
            (Some(_), Some(_)) => {
                panic!(
                    "only one of password or key may be specified per repository ({:?})",
                    repo_name
                );
            }
            (Some(k), None) => Ok(Some(k)),
            (None, Some(p)) => Ok(Some(p)),
            (None, None) => Ok(None),
        }
    }
}

/// Colon-separated name-value pair.
#[derive(Debug)]
pub(crate) struct Named<T> {
    pub name: String,
    pub value: T,
}

impl<T> FromStr for Named<T>
where
    T: FromStr,
    Error: From<T::Err>,
{
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let index = input.find(':').context("missing ':'")?;

        Ok(Self {
            name: input[..index].to_owned(),
            value: input[index + 1..].parse()?,
        })
    }
}

*/

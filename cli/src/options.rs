// TODO: remove this when we upgrade clap to v4.0
#![allow(deprecated)]

use crate::APP_NAME;
use anyhow::{format_err, Context, Error, Result};
use clap::Parser;
use ouisync_lib::{
    crypto::{cipher::SecretKey, Password},
    network::NetworkOptions,
    AccessMode, MasterSecret, ShareToken,
};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
};

/// Command line options.
#[derive(Parser, Debug)]
pub(crate) struct Options {
    /// Path to the data directory. Use the --print-dirs flag to see the default.
    #[clap(long, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// Path to the config directory. Use the --print-dirs flag to see the default.
    #[clap(long, value_name = "PATH")]
    pub config_dir: Option<PathBuf>,

    /// Disable Merger (experimental, will likely be removed in the future)
    #[clap(long)]
    pub disable_merger: bool,

    #[clap(flatten)]
    pub network: NetworkOptions,

    /// Disable DHT
    #[clap(long)]
    pub disable_dht: bool,

    /// Create new repository with the specified name. Can be specified multiple times to create
    /// multiple repositories.
    #[clap(short, long, value_name = "NAME")]
    pub create: Vec<String>,

    /// Mount the named repository at the specified path. Can be specified multiple times to mount
    /// multiple repositories.
    #[clap(short, long, value_name = "NAME:PATH")]
    pub mount: Vec<Named<PathBuf>>,

    /// Password per repository.
    // TODO: Zeroize
    #[clap(long, value_name = "NAME:PASSWD")]
    pub password: Vec<Named<String>>,

    /// Pre-hashed 32 byte long (64 hexadecimal characters) master secret per repository. This is
    /// mainly intended for testing as password derivation is rather slow and some of the tests may
    /// timeout if the `password` argument is used instead. For all other use cases, prefer to use
    /// the `password` argument instead.
    // TODO: Zeroize
    #[clap(long, value_name = "NAME:KEY")]
    pub key: Vec<Named<String>>,

    /// Print share token for the named repository with the specified access mode ("blind", "read"
    /// or "write"). Can be specified multiple times to share multiple repositories.
    #[clap(long, value_name = "NAME:ACCESS_MODE")]
    pub share: Vec<Named<AccessMode>>,

    /// Print the share tokens to a file instead of standard output (one token per line)
    #[clap(long, value_name = "PATH")]
    pub share_file: Option<PathBuf>,

    /// Accept a share token. Can be specified multiple times to accept multiple tokens.
    #[clap(long, value_name = "TOKEN")]
    pub accept: Vec<ShareToken>,

    /// Accept share tokens by reading them from a file, one token per line.
    #[clap(long, value_name = "PATH")]
    pub accept_file: Option<PathBuf>,

    /// Prints the path to the data and config directories and exits.
    #[clap(long)]
    pub print_dirs: bool,

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
    /// Path to the data directory.
    pub fn data_dir(&self) -> Result<PathBuf> {
        if let Some(path) = &self.data_dir {
            Ok(path.clone())
        } else {
            Ok(dirs::data_dir()
                .context("failed to initialize default data directory")?
                .join(APP_NAME))
        }
    }

    /// Path to the config directory.
    pub fn config_dir(&self) -> Result<PathBuf> {
        if let Some(path) = &self.config_dir {
            Ok(path.clone())
        } else {
            Ok(dirs::config_dir()
                .context("failed to initialize default config directory")?
                .join(APP_NAME))
        }
    }

    /// Path to the database of the repository with the specified name.
    pub fn repository_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self
            .data_dir()?
            .join("repositories")
            .join(name)
            .with_extension("db"))
    }

    pub fn secret_for_repo(&self, repo_name: &str) -> Result<MasterSecret> {
        let key = self
            .key
            .iter()
            .find(|e| e.name == repo_name)
            .map(|e| e.value.as_str())
            .map(|k| {
                MasterSecret::SecretKey(SecretKey::parse_hex(k).expect("failed to parse key"))
            });

        let pwd = self
            .password
            .iter()
            .find(|e| e.name == repo_name)
            .map(|e| e.value.as_str())
            .map(|k| MasterSecret::Password(Password::new(k)));

        match (key, pwd) {
            (Some(_), Some(_)) => {
                panic!(
                    "only one of password or key may be specified per repository ({:?})",
                    repo_name
                );
            }
            (Some(k), None) => Ok(k),
            (None, Some(p)) => Ok(p),
            (None, None) => Err(format_err!(
                "missing password or key for repository {:?}",
                repo_name
            )),
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

pub(crate) async fn read_share_tokens_from_file(path: &Path) -> Result<Vec<ShareToken>> {
    let file = File::open(path).await?;
    let mut reader = BufReader::new(file);
    let mut buffer = String::new();
    let mut tokens = Vec::new();

    while reader.read_line(&mut buffer).await? > 0 {
        let token: ShareToken = buffer.parse()?;
        tokens.push(token);
        buffer.clear();
    }

    Ok(tokens)
}

use crate::APP_NAME;
use anyhow::{Context, Error, Result};
use ouisync_lib::{NetworkOptions, ShareToken, Store};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};
use structopt::StructOpt;
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
};

/// Command line options.
#[derive(StructOpt, Debug)]
pub(crate) struct Options {
    /// Path to the data directory. Use the --print-data-dir flag to see the default.
    #[structopt(long, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// Disable Merger
    #[structopt(long)]
    pub disable_merger: bool,

    #[structopt(flatten)]
    pub network: NetworkOptions,

    /// Mount the named repository at the specified path. If no such repository exists yet, it will
    /// be created. Can be specified multiple times to mount multiple repositories.
    #[structopt(short, long, value_name = "NAME:PATH")]
    pub mount: Vec<Named<PathBuf>>,

    /// Password per repository.
    #[structopt(long, value_name = "NAME:PASSWD")]
    pub password: Vec<Named<String>>,

    /// Print share token for the named repository. Can be specified multiple times to share
    /// multiple repositories.
    #[structopt(long, value_name = "NAME")]
    pub share: Vec<String>,

    /// Print the share tokens to a file instead of standard output (one token per line)
    #[structopt(long, value_name = "PATH")]
    pub share_file: Option<PathBuf>,

    /// Accept a share token. Can be specified multiple times to accept multiple tokens.
    #[structopt(long, value_name = "TOKEN")]
    pub accept: Vec<ShareToken>,

    /// Accept share tokens by reading them from a file, one token per line.
    #[structopt(long, value_name = "PATH")]
    pub accept_file: Option<PathBuf>,

    /// Prints the path to the data directory and exits.
    #[structopt(long)]
    pub print_data_dir: bool,

    /// Prints the listening address to the stdout when the replica becomes ready.
    /// Note this flag is unstable and experimental.
    #[structopt(long)]
    pub print_ready_message: bool,

    /// Use temporary, memory-only databases. All data will be wiped out when the program
    /// exits. If this flag is set, the --data-dir option is ignored. Use only for experimentation
    /// and testing.
    #[structopt(long)]
    pub temp: bool,
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

    /// Path to the config database.
    pub fn config_path(&self) -> Result<PathBuf> {
        Ok(self.data_dir()?.join("config.db"))
    }

    /// Store of the config database
    pub fn config_store(&self) -> Result<Store> {
        if self.temp {
            Ok(Store::Memory)
        } else {
            Ok(Store::File(self.config_path()?))
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

    /// Store of the database of the repository with the specified name.
    pub fn repository_store(&self, name: &str) -> Result<Store> {
        if self.temp {
            Ok(Store::Memory)
        } else {
            Ok(Store::File(self.repository_path(name)?))
        }
    }

    pub fn password_for_repo(&self, repo_name: &str) -> Option<&String> {
        for Named { name, value } in &self.password {
            if repo_name == name {
                return Some(value);
            }
        }
        return None;
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

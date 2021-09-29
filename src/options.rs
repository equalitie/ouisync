use anyhow::{Context, Result};
use ouisync::{NetworkOptions, Store};
use std::{path::PathBuf, str::FromStr};
use structopt::StructOpt;
use thiserror::Error;

/// Command line options.
#[derive(StructOpt, Debug)]
pub(crate) struct Options {
    /// Path to the data directory. Use the --print-data-dir flag to see the default.
    #[structopt(long, value_name = "PATH")]
    pub data_dir: Option<PathBuf>,

    /// Path to the config database [default: "DATA_DIR/config.db"]
    #[structopt(long, value_name = "PATH")]
    pub config_path: Option<PathBuf>,

    /// Disable Merger
    #[structopt(long)]
    pub disable_merger: bool,

    #[structopt(flatten)]
    pub network: NetworkOptions,

    /// Mount the named repository at the specified path. Can be specified multiple times to mount
    /// multiple repositories. If no such repository exists yet, it will be created.
    #[structopt(short, long, value_name = "NAME:PATH")]
    pub mount: Vec<MountPoint>,

    /// Prints the path to the data directory and exits.
    #[structopt(long)]
    pub print_data_dir: bool,

    /// Prints the listening address to the stdout when the replica becomes ready.
    /// Note this flag is unstable and experimental.
    #[structopt(long)]
    pub print_ready_message: bool,

    /// Use temporary, memory-only databases. All data will be wiped out when the program
    /// exits. If this flag is set, the --data-dir and --config-path options are ignored.
    /// Use only for experimentation and testing.
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
                .join(env!("CARGO_PKG_NAME")))
        }
    }

    /// Path to the config database.
    pub fn config_path(&self) -> Result<PathBuf> {
        if let Some(path) = &self.config_path {
            Ok(path.clone())
        } else {
            Ok(self.data_dir()?.join("config.db"))
        }
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
}

/// Specification of a repository mount point.
#[derive(Debug)]
pub(crate) struct MountPoint {
    /// Name of the repository to be mounted.
    pub name: String,
    /// Path to the directory to mount the repository to.
    pub path: PathBuf,
}

impl FromStr for MountPoint {
    type Err = MountPointParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let index = input.find(':').ok_or(MountPointParseError)?;

        Ok(Self {
            name: input[..index].to_owned(),
            path: input[index + 1..].into(),
        })
    }
}

#[derive(Debug, Error)]
#[error("invalid mount point specification")]
pub(crate) struct MountPointParseError;

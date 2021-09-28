use anyhow::{Context, Result};
use ouisync::NetworkOptions;
use std::{convert::Infallible, path::PathBuf, str::FromStr};
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
    /// multiple repositories.
    #[structopt(short, long, value_name = "NAME:PATH")]
    pub mount: Vec<MountPoint>,

    /// Create repository. Can optionally specify the path to the repository database or leave it
    /// out to use the default: "DATA_DIR/repositories/NAME.db". Can be specified multiple times to
    /// create multiple repositories.
    #[structopt(long, value_name = "NAME|NAME:PATH")]
    pub create_repository: Vec<CreateRepository>,

    /// Delete repository. WARNING: this is permanent and irreversible. Use with caution! Can be
    /// specified multiple times to delete multiple repositories.
    #[structopt(long, value_name = "NAME")]
    pub delete_repository: Vec<String>,

    /// Prints the path to the data directory and exits.
    #[structopt(long)]
    pub print_data_dir: bool,

    /// Prints the listening address to the stdout when the replica becomes ready.
    /// Note this flag is unstable and experimental.
    #[structopt(long)]
    pub print_ready_message: bool,
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

    /// Default path to the repository databases.
    pub fn default_repository_path(&self, name: &str) -> Result<PathBuf> {
        // TODO: escape "/" in the name (possibly other characters too)
        Ok(self
            .data_dir()?
            .join("repositories")
            .join(name)
            .with_extension("db"))
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

/// Parameters to create new repository.
#[derive(Debug)]
pub(crate) struct CreateRepository {
    /// Name of the repository to create.
    pub name: String,
    /// Path to store the repository database at or `None` to use the default one
    /// ("$DATA_DIR/repositories/$NAME.db")
    pub path: Option<PathBuf>,
}

impl FromStr for CreateRepository {
    type Err = Infallible;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if let Some(index) = input.find(':') {
            Ok(Self {
                name: input[..index].to_owned(),
                path: Some(input[index + 1..].into()),
            })
        } else {
            Ok(Self {
                name: input.to_owned(),
                path: None,
            })
        }
    }
}

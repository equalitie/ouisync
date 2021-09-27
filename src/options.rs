use anyhow::{Context, Result};
use ouisync::NetworkOptions;
use std::path::PathBuf;
use structopt::StructOpt;

/// Command line options.
#[derive(StructOpt)]
pub(crate) struct Options {
    /// Path to the config database.
    #[structopt(long, value_name = "PATH")]
    pub config_path: Option<PathBuf>,

    /// Path to the directory containing repositories.
    #[structopt(long, value_name = "DIR")]
    pub repository_dir: Option<PathBuf>,

    /// Disable Merger
    #[structopt(long)]
    pub disable_merger: bool,

    #[structopt(flatten)]
    pub network: NetworkOptions,

    /// Create repository.
    #[structopt(long, value_name = "NAME|NAME:PATH")]
    pub create_repository: Vec<String>,

    /// Delete repository. WARNING: this permanently and irreversibly deletes the specified
    /// repositories. Use with caution!
    #[structopt(long, value_name = "NAME")]
    pub delete_repository: Vec<String>,

    /// Mount the named repository at the specified path.
    #[structopt(short, long, value_name = "NAME:PATH")]
    pub mount_point: Vec<String>,

    /// Print the listening address to the stdout when the replica becomes ready.
    /// Note this flag is unstable and experimental.
    #[structopt(long)]
    pub print_ready_message: bool,
}

impl Options {
    /// Path to the config database
    pub fn config_path(&self) -> Result<PathBuf> {
        if let Some(path) = &self.config_path {
            Ok(path.clone())
        } else {
            Ok(self.data_dir()?.join("config.db"))
        }
    }

    pub fn repository_dir(&self) -> Result<PathBuf> {
        if let Some(path) = &self.repository_dir {
            Ok(path.clone())
        } else {
            Ok(self.data_dir()?.join("repositories"))
        }
    }

    fn data_dir(&self) -> Result<PathBuf> {
        Ok(dirs::data_dir()
            .context("failed to initialize data directory")?
            .join(env!("CARGO_PKG_NAME")))
    }
}

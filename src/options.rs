use anyhow::{Context, Result};
use ouisync::NetworkOptions;
use std::path::PathBuf;
use structopt::StructOpt;

/// Command line options.
#[derive(StructOpt)]
pub(crate) struct Options {
    /// Database name
    #[structopt(short = "n", long, default_value = "db")]
    pub db_name: String,

    /// Databse directory
    #[structopt(short, long)]
    pub data_dir: Option<PathBuf>,

    /// Mount directory
    #[structopt(short, long)]
    pub mount_dir: PathBuf,

    /// Disable Merger
    #[structopt(long)]
    pub disable_merger: bool,

    #[structopt(flatten)]
    pub network: NetworkOptions,

    /// Print the listening address to the stdout when the replica becomes ready.
    /// Note this flag is unstable and experimental.
    #[structopt(long)]
    pub print_ready_message: bool,
}

impl Options {
    // Path to the database.
    pub fn db_path(&self) -> Result<PathBuf> {
        Ok(self.data_dir()?.join(&self.db_name))
    }

    pub fn data_dir(&self) -> Result<PathBuf> {
        if let Some(data_dir) = &self.data_dir {
            return Ok(data_dir.clone());
        }

        Ok(dirs::data_dir()
            .context("failed to initialize data directory")?
            .join(env!("CARGO_PKG_NAME")))
    }
}

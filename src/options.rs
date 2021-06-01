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

    /// Mount directory
    #[structopt(short, long)]
    pub mount_dir: PathBuf,

    #[structopt(flatten)]
    pub network: NetworkOptions,
}

impl Options {
    // Path to the database.
    pub fn db_path(&self) -> Result<PathBuf> {
        Ok(dirs::data_dir()
            .context("failed to initialize data directory")?
            .join(env!("CARGO_PKG_NAME"))
            .join(&self.db_name))
    }
}

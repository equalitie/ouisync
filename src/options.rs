use anyhow::{Context, Result};
use ouisync::NetworkOptions;
use std::{net::SocketAddr, path::PathBuf};
use structopt::StructOpt;

/// Command line options.
#[derive(StructOpt)]
pub(crate) struct Options {
    /// Mount directory
    #[structopt(short, long)]
    pub mount_dir: PathBuf,

    /// Peer's endpoint
    #[structopt(short, long, value_name = "ip:port")]
    pub connect: Option<SocketAddr>,

    #[structopt(flatten)]
    pub network: NetworkOptions,
}

impl Options {
    // Path to the database.
    pub fn db_path(&self) -> Result<PathBuf> {
        Ok(dirs::data_dir()
            .context("failed to initialize data directory")?
            .join(env!("CARGO_PKG_NAME"))
            .join("db"))
    }
}

use anyhow::{Context, Result};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
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

    /// Port to listen on
    #[structopt(short, long, default_value = "65535")]
    pub port: u16,

    /// IP address to bind to
    #[structopt(long, default_value = "0.0.0.0")]
    pub bind: IpAddr,
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

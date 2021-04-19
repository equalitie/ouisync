mod virtual_filesystem;

use anyhow::Result;
use ouisync::Repository;
use ouisync::ReplicaDiscovery;
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
use structopt::StructOpt;
use tokio::signal;
use async_std;

#[derive(StructOpt)]
struct Options {
    /// Mount directory
    #[structopt(short, long)]
    mount_dir: PathBuf,

    /// Base directory
    #[structopt(short, long)]
    base_dir: Option<PathBuf>,

    /// Peer's endpoint
    #[structopt(short, long, value_name = "ip:port")]
    connect: Option<SocketAddr>,

    /// Port to listen on
    #[structopt(short, long, default_value = "65535")]
    port: u16,

    /// IP address to bind to
    #[structopt(long, default_value = "0.0.0.0")]
    bind: IpAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let repository = Repository;
    let _mount_guard = virtual_filesystem::mount(repository, options.mount_dir)?;

    let listener = async_std::net::TcpListener::bind(SocketAddr::from(([0,0,0,0], 0))).await?;
    async_std::task::spawn(ReplicaDiscovery::new(listener.local_addr().unwrap())?.run());

    signal::ctrl_c().await?;

    Ok(())
}

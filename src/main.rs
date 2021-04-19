mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{db, BlockStore, ReplicaDiscovery};
use structopt::StructOpt;
use tokio::signal;

async fn run_local_discovery() -> std::io::Result<()> {
    use std::net::SocketAddr;
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0,0,0,0], 0))).await?;

    let mut discovery = ReplicaDiscovery::new(listener.local_addr().unwrap())?;
    
    loop {
        let found = discovery.wait_for_activity().await;
        println!("found: {:?}", found);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let pool = db::init(options.db_path()?).await?;
    let _block_store = BlockStore::open(pool).await?;

    // let repository = Repository;
    // let _mount_guard = virtual_filesystem::mount(repository, options.mount_dir)?;

    if options.enable_local_discovery {
        tokio::task::spawn(run_local_discovery());
    }

    signal::ctrl_c().await?;

    Ok(())
}

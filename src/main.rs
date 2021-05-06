mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{db, this_replica, Network};
use structopt::StructOpt;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let pool = db::init(options.db_path()?).await?;

    let _replica_id = this_replica::get_or_create_id(&pool).await?;

    let _network = Network::new(options.enable_local_discovery);

    // let repository = Repository;
    // let _mount_guard = virtual_filesystem::mount(repository, options.mount_dir)?;

    signal::ctrl_c().await?;

    Ok(())
}

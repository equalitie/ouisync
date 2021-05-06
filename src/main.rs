mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{db, this_replica, Cryptor, Network, Repository};
use structopt::StructOpt;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let pool = db::init(options.db_path()?).await?;
    let replica_id = this_replica::get_or_create_id(&pool).await?;
    let cryptor = Cryptor::Null; // TODO:

    let _network = Network::new(options.enable_local_discovery);

    let repository = Repository::new(pool, replica_id, cryptor);
    let _mount_guard = virtual_filesystem::mount(
        tokio::runtime::Handle::current(),
        repository,
        options.mount_dir,
    )?;

    signal::ctrl_c().await?;

    Ok(())
}

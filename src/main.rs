mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{db, Cryptor, Network, Repository};
use structopt::StructOpt;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let pool = db::init(db::Store::File(options.db_path()?)).await?;
    let cryptor = Cryptor::Null; // TODO:

    let _network = Network::new(options.enable_local_discovery);

    let repository = Repository::new(pool, cryptor).await?;
    let _mount_guard = virtual_filesystem::mount(
        tokio::runtime::Handle::current(),
        repository,
        options.mount_dir,
    )?;

    signal::ctrl_c().await?;

    Ok(())
}

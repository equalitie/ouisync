mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{db, Cryptor, Session};
use structopt::StructOpt;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    env_logger::init();

    let session = Session::new(db::Store::File(options.db_path()?), Cryptor::Null, true).await?;

    let _mount_guard = virtual_filesystem::mount(
        tokio::runtime::Handle::current(),
        session.open_repository(),
        options.mount_dir,
    )?;

    signal::ctrl_c().await?;

    Ok(())
}

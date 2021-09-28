mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{Session, Store};
use structopt::StructOpt;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<()> {
    let options = Options::from_args();

    if options.print_data_dir {
        println!("{}", options.data_dir()?.display());
        return Ok(());
    }

    env_logger::init();

    let session = Session::new(
        Store::from(options.config_path()?),
        !options.disable_merger,
        options.network,
    )
    .await?;

    // let _mount_guard = virtual_filesystem::mount(
    //     tokio::runtime::Handle::current(),
    //     session.open_repository(!options.disable_merger),
    //     options.mount_dir,
    // )?;

    if options.print_ready_message {
        println!("Listening on port {}", session.local_addr().port());
        println!(
            "This replica ID is {}",
            session.repositories().this_replica_id()
        );
    }

    signal::ctrl_c().await?;

    Ok(())
}

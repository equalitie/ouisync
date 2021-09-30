mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{config, this_replica, Cryptor, Network, Repository};
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

    let pool = config::open_db(&options.config_store()?).await?;
    let this_replica_id = this_replica::get_or_create_id(&pool).await?;

    // Start the network
    let network = Network::new(this_replica_id, &options.network).await?;
    let network_handle = network.handle();

    // Mount repositories
    let mut mount_guards = Vec::new();
    for mount_point in &options.mount {
        let repo = Repository::open(
            &options.repository_store(&mount_point.name)?,
            this_replica_id,
            Cryptor::Null,
            !options.disable_merger,
        )
        .await?;

        network_handle.register(&mount_point.name, &repo).await;

        let guard = virtual_filesystem::mount(
            tokio::runtime::Handle::current(),
            repo,
            mount_point.path.clone(),
        )?;

        mount_guards.push(guard);
    }

    if options.print_ready_message {
        println!("Listening on port {}", network.local_addr().port());
        println!("This replica ID is {}", this_replica_id);
    }

    signal::ctrl_c().await?;

    Ok(())
}

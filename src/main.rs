mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync::{config, this_replica, Cryptor, Network, Repository};
use std::collections::HashMap;
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

    let pool = config::open_db(options.config_path()?.into()).await?;
    let this_replica_id = this_replica::get_or_create_id(&pool).await?;

    let mut repositories = HashMap::new();

    // Create repositories
    for name in &options.create_repository {
        let repo = Repository::open(
            options.repository_path(name)?.into(),
            this_replica_id,
            Cryptor::Null,
            !options.disable_merger,
        )
        .await?;
        repositories.insert(name.clone(), repo);
    }

    // Start the network
    let network = Network::new(this_replica_id, &options.network).await?;

    // Mount repositories
    let mut mount_guards = Vec::new();
    for mount_point in &options.mount {
        let repo = if let Some(repo) = repositories.remove(&mount_point.name) {
            repo
        } else {
            Repository::open(
                options.repository_path(&mount_point.name)?.into(),
                this_replica_id,
                Cryptor::Null,
                !options.disable_merger,
            )
            .await?
        };

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

mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::{Context, Result};
use ouisync::{config, Cryptor, Network, RepositoryManager};
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
    let mut repositories = RepositoryManager::load(pool, !options.disable_merger).await?;

    // Create repositories
    for create in &options.create_repository {
        let path = if let Some(path) = &create.path {
            path.clone()
        } else {
            options.default_repository_path(&create.name)?
        };

        repositories
            .create(create.name.clone(), path.into(), Cryptor::Null)
            .await?;
    }

    // Delete repositories
    for name in &options.delete_repository {
        repositories.delete(name).await?
    }

    // Mount repositories
    let mut mount_guards = Vec::new();
    for mount_point in &options.mount {
        let repository = repositories.get(&mount_point.name).with_context(|| {
            format!(
                "can't mount repository {:?} - no such repository",
                mount_point.name
            )
        })?;

        let guard = virtual_filesystem::mount(
            tokio::runtime::Handle::current(),
            repository.clone(),
            mount_point.path.clone(),
        )?;

        mount_guards.push(guard);
    }

    // Start the network
    let network = Network::new(repositories.subscribe(), options.network).await?;

    if options.print_ready_message {
        println!("Listening on port {}", network.local_addr().port());
        println!("This replica ID is {}", repositories.this_replica_id());
    }

    signal::ctrl_c().await?;

    Ok(())
}

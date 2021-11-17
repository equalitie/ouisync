mod options;
mod virtual_filesystem;

use self::options::Options;
use anyhow::Result;
use ouisync_lib::{config, this_replica, Cryptor, Network, Repository};
use std::io;
use structopt::StructOpt;

pub(crate) const APP_NAME: &str = "ouisync";

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
            mount_point.name.to_owned(),
            &options.repository_store(&mount_point.name)?,
            this_replica_id,
            Cryptor::Null,
            !options.disable_merger,
        )
        .await?;

        network_handle.register(&repo).await;

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

    terminated().await?;

    Ok(())
}

// Wait until the program is terminated.
#[cfg(unix)]
async fn terminated() -> io::Result<()> {
    use tokio::{
        select,
        signal::unix::{signal, SignalKind},
    };

    // Wait for SIGINT or SIGTERM
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;

    select! {
        _ = interrupt.recv() => (),
        _ = terminate.recv() => (),
    }

    Ok(())
}

#[cfg(not(unix))]
async fn terminated() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}

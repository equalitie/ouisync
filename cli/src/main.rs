mod options;
mod virtual_filesystem;

use self::options::{Named, Options};
use anyhow::Result;
use ouisync_lib::{config, this_replica, Cryptor, Network, Repository, ShareToken};
use std::{collections::HashMap, io};
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

    // Gather the repositories to be mounted.
    let mut mount_repos = HashMap::new();
    for Named { name, value } in &options.mount {
        let repo = Repository::open(
            &options.repository_store(name)?,
            this_replica_id,
            Cryptor::Null,
            !options.disable_merger,
        )
        .await?;

        mount_repos.insert(name.as_str(), (repo, value));
    }

    // Print repository share tokens
    for name in &options.share {
        if let Some((repo, _)) = mount_repos.get(name.as_str()) {
            print_share_token(repo, name).await?
        } else {
            print_share_token(
                &Repository::open(
                    &options.repository_store(name)?,
                    this_replica_id,
                    Cryptor::Null,
                    false,
                )
                .await?,
                name,
            )
            .await?
        }
    }

    // Accept share tokens
    for Named { name, value } in &options.accept {
        if let Some((repo, _)) = mount_repos.get(name.as_str()) {
            accept_share_token(repo, value).await?
        } else {
            accept_share_token(
                &Repository::open(
                    &options.repository_store(name)?,
                    this_replica_id,
                    Cryptor::Null,
                    false,
                )
                .await?,
                value,
            )
            .await?
        }
    }

    // Start the network
    let network = Network::new(this_replica_id, &options.network).await?;
    let network_handle = network.handle();

    // Mount repositories
    let mut mount_guards = Vec::new();
    for (repo, mount_point) in mount_repos.into_values() {
        network_handle.register(&repo).await;

        let guard = virtual_filesystem::mount(
            tokio::runtime::Handle::current(),
            repo,
            mount_point.clone(),
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

async fn print_share_token(repo: &Repository, name: &str) -> Result<()> {
    println!("{}", repo.share().await?.with_name(name));
    Ok(())
}

async fn accept_share_token(repo: &Repository, token: &ShareToken) -> Result<()> {
    repo.accept(token).await?;
    log::info!("share token accepted: {}", token);
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

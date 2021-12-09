mod options;
mod virtual_filesystem;

use self::options::{Named, Options};
use anyhow::Result;
use ouisync_lib::{config, this_replica, Cryptor, Network, Repository, ShareToken};
use std::{collections::HashMap, io};
use structopt::StructOpt;
use tokio::{fs::File, io::AsyncWriteExt};

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
    let this_writer_id = this_replica::get_or_create_id(&pool).await?;

    // Gather the repositories to be mounted.
    let mut mount_repos = HashMap::new();
    for Named { name, value } in &options.mount {
        let repo = Repository::open(
            &options.repository_store(name)?,
            this_writer_id,
            Cryptor::Null,
            !options.disable_merger,
        )
        .await?;

        mount_repos.insert(name.as_str(), (repo, value));
    }

    // Print share tokens
    let mut share_file = if let Some(path) = &options.share_file {
        Some(File::create(path).await?)
    } else {
        None
    };

    for name in &options.share {
        let id = if let Some((repo, _)) = mount_repos.get(name.as_str()) {
            repo.get_or_create_id().await?
        } else {
            Repository::open(
                &options.repository_store(name)?,
                this_writer_id,
                Cryptor::Null,
                false,
            )
            .await?
            .get_or_create_id()
            .await?
        };

        let token = ShareToken::new(id).with_name(name);

        if let Some(file) = &mut share_file {
            file.write_all(token.to_string().as_bytes()).await?;
            file.write(b"\n").await?;
        } else {
            println!("{}", token);
        }
    }

    if let Some(mut file) = share_file {
        file.flush().await?;
    }

    // Accept share tokens
    let accept_file_tokens = if let Some(path) = &options.accept_file {
        options::read_share_tokens_from_file(path).await?
    } else {
        vec![]
    };

    for token in options.accept.iter().chain(&accept_file_tokens) {
        let name = token.suggested_name();

        if let Some((repo, _)) = mount_repos.get(name.as_ref()) {
            repo.set_id(*token.id()).await?
        } else {
            Repository::open(
                &options.repository_store(name.as_ref())?,
                this_writer_id,
                Cryptor::Null,
                false,
            )
            .await?
            .set_id(*token.id())
            .await?
        }

        log::info!("share token accepted: {}", token);
    }

    // Start the network
    let network = Network::new(&options.network).await?;
    let network_handle = network.handle();

    // Mount repositories
    let mut repo_guards = Vec::new();
    for (repo, mount_point) in mount_repos.into_values() {
        let registration = network_handle.register(&repo).await;
        let mount_guard = virtual_filesystem::mount(
            tokio::runtime::Handle::current(),
            repo,
            mount_point.clone(),
        )?;

        repo_guards.push((mount_guard, registration));
    }

    if options.print_ready_message {
        println!("Listening on port {}", network.local_addr().port());
        println!("This writer ID is {}", this_writer_id);
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

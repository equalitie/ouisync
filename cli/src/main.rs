mod options;
mod virtual_filesystem;

use self::options::{Named, Options};
use anyhow::{format_err, Result};
use ouisync_lib::{config, replica_id, AccessSecrets, Network, Repository, ShareToken};
use std::{
    collections::{hash_map::Entry, HashMap},
    io,
};
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
    let this_writer_id = replica_id::get_or_create_this_replica_id(&pool).await?;

    // Create repositories
    let mut repos = HashMap::new();

    for name in &options.create {
        let secret = options.secret_for_repo(name)?;
        let repo = Repository::create(
            &options.repository_store(name)?,
            this_writer_id,
            secret,
            AccessSecrets::random_write(),
            !options.disable_merger,
        )
        .await?;

        repos.insert(name.clone(), repo);
    }

    // Print share tokens
    let mut share_file = if let Some(path) = &options.share_file {
        Some(File::create(path).await?)
    } else {
        None
    };

    for Named { name, value } in &options.share {
        let secrets = if let Some(repo) = repos.get(name) {
            repo.secrets().with_mode(*value)
        } else {
            Repository::open(
                &options.repository_store(name)?,
                this_writer_id,
                options.secret_for_repo(name).ok(),
                false,
            )
            .await?
            .secrets()
            .with_mode(*value)
        };

        let token = ShareToken::from(secrets).with_name(name);

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
        let access_secrets = token.secrets();
        let master_secret = options.secret_for_repo(&name)?;

        if let Entry::Vacant(entry) = repos.entry(name.as_ref().to_owned()) {
            let repo = Repository::create(
                &options.repository_store(name.as_ref())?,
                this_writer_id,
                master_secret,
                access_secrets.clone(),
                false,
            )
            .await?;

            entry.insert(repo);

            log::info!("share token accepted: {}", token);
        } else {
            return Err(format_err!(
                "can't accept share token for repository {:?} - already exists",
                name
            ));
        }
    }

    // Start the network
    let network = Network::new(&options.network).await?;
    let network_handle = network.handle();

    // Mount repositories
    let mut repo_guards = Vec::new();
    for Named { name, value } in &options.mount {
        let repo = if let Some(repo) = repos.remove(name) {
            repo
        } else {
            Repository::open(
                &options.repository_store(name)?,
                this_writer_id,
                options.secret_for_repo(name).ok(),
                !options.disable_merger,
            )
            .await?
        };

        let registration = network_handle.register(&repo).await;
        let mount_guard =
            virtual_filesystem::mount(tokio::runtime::Handle::current(), repo, value.clone())?;

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

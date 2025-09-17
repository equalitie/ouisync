use crate::{defaults, migration, options::ServerCommand};
use ouisync_service::{logger, Error, Service};
use std::{io, path::PathBuf};
use tokio::select;

pub(crate) async fn run(config_dir: PathBuf, command: ServerCommand) -> Result<(), Error> {
    let ServerCommand::Start {
        log_format,
        log_color,
    } = command;

    logger::init(log_format, log_color);

    if config_dir == defaults::config_dir() {
        migration::migrate_config_dir().await;
    }

    let mut service = Service::init(config_dir).await?;

    let store_dirs = service.store_dirs();
    let store_dirs = if store_dirs.is_empty() {
        let dir = defaults::store_dir();
        service.set_store_dirs(vec![dir.clone()]).await?;
        vec![dir]
    } else {
        store_dirs
    };

    migration::check_store_dir(&store_dirs).await;

    service.init_network().await;
    service.set_sync_enabled_all(true).await?;

    select! {
        result = service.run() => match result {
            Err(error) => Err(error)?,
        },
        result = terminated() => result?,
    };

    service.close().await;

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

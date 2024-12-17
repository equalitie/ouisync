use crate::{defaults, options::ServerCommand};
use ouisync_service::{logger::Logger, Error, Service};
use std::{io, path::PathBuf};
use tokio::select;

pub(crate) async fn run(socket: PathBuf, command: ServerCommand) -> Result<(), Error> {
    let ServerCommand::Start {
        config_dir,
        log_format,
        log_color,
    } = command;

    let _logger = Logger::new(
        None,
        String::new(), // log tag, not used here
        log_format,
        log_color,
    )?;

    let mut service = Service::init(socket, config_dir).await?;

    if service.store_dir().is_none() {
        service.set_store_dir(defaults::store_dir()).await?;
    }

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

use crate::{handler::LocalHandler, options::Dirs, state::State, transport::local::LocalServer};
use anyhow::Result;
use ouisync_bridge::logger;
use ouisync_lib::StateMonitor;
use std::{io, path::Path, sync::Arc};
use tokio::task;

pub(crate) async fn run(dirs: Dirs, host: String) -> Result<()> {
    let monitor = StateMonitor::make_root();
    let _logger = logger::new(Some(monitor.clone()));

    let state = State::new(&dirs, monitor).await;
    let state = Arc::new(state);

    let server = LocalServer::bind(Path::new(&host))?;
    let handle = task::spawn(server.run(LocalHandler::new(state.clone())));

    terminated().await?;

    handle.abort();
    state.close().await;

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

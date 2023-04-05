use crate::{
    handler::{Handler, State},
    host_addr::HostAddr,
    options::Options,
    transport::local::LocalServer,
};
use anyhow::{format_err, Result};
use ouisync_bridge::logger;
use ouisync_lib::StateMonitor;
use std::{io, sync::Arc};
use tokio::task;

pub(crate) async fn run(options: Options) -> Result<()> {
    let monitor = StateMonitor::make_root();
    let _logger = logger::new(Some(monitor.clone()));

    let state = State::new(&options.dirs, monitor).await;
    let state = Arc::new(state);

    let addr = match options.host {
        HostAddr::Local(addr) => addr,
        HostAddr::Remote(_) => {
            return Err(format_err!("remote api endpoints not supported yet"));
        }
    };

    let server = LocalServer::bind(addr.clone())?;
    let handle = task::spawn(server.run(Handler::new(state.clone())));

    tracing::info!("API server listening on {}", addr);

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

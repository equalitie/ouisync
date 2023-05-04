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

    let mut server_handles = Vec::new();

    let server = LocalServer::bind(Path::new(&host))?;
    tracing::info!("API server listening on {}", host);

    let handle = task::spawn(server.run(LocalHandler::new(state.clone())));
    server_handles.push(handle);

    // HostAddr::Remote(addr) => {
    //     let server = RemoteServer::bind(*addr).await?;
    //     tracing::info!("API server listening on {}", server.local_addr());

    //     task::spawn(server.run(RemoteHandler::new(state.clone())))
    // }

    terminated().await?;

    for handle in server_handles {
        handle.abort();
    }

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

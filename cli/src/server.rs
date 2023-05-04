use crate::{
    handler::Handler,
    host_addr::HostAddr,
    options::Dirs,
    state::State,
    transport::{local::LocalServer, remote::RemoteServer},
};
use anyhow::{format_err, Result};
use ouisync_bridge::logger;
use ouisync_lib::StateMonitor;
use std::{io, net::SocketAddr, sync::Arc};
use tokio::task;

pub(crate) async fn run(dirs: Dirs, hosts: Vec<String>) -> Result<()> {
    let hosts: Vec<HostAddr<SocketAddr>> = hosts
        .into_iter()
        .map(|host| Ok(host.parse()?))
        .collect::<Result<_>>()?;

    if hosts.is_empty() {
        return Err(format_err!("host required"));
    }

    let monitor = StateMonitor::make_root();
    let _logger = logger::new(Some(monitor.clone()));

    let state = State::new(&dirs, monitor).await;
    let state = Arc::new(state);

    let mut server_handles = Vec::new();

    for host in hosts {
        let handle = match &host {
            HostAddr::Local(path) => {
                let server = LocalServer::bind(path.as_path())?;
                task::spawn(server.run(Handler::new(state.clone())))
            }
            HostAddr::Remote(addr) => {
                let server = RemoteServer::bind(*addr).await?;
                task::spawn(server.run(Handler::new(state.clone())))
            }
        };

        server_handles.push(handle);

        tracing::info!("API server listening on {}", host);
    }

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

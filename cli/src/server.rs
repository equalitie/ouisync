use crate::{
    handler::{Handler, State},
    host_addr,
    options::Options,
    transport::local::LocalServer,
};
use ouisync_bridge::logger;
use ouisync_lib::StateMonitor;
use std::{io, sync::Arc};
use tokio::task;

pub(crate) async fn run(options: Options) -> anyhow::Result<()> {
    let root_monitor = StateMonitor::make_root();
    let _logger = logger::new(root_monitor.clone());

    let state = State::new(&options.dirs);
    let state = Arc::new(state);

    let server = LocalServer::bind(host_addr::default_local())?;
    let handle = task::spawn(server.run(Handler::new(state.clone())));

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

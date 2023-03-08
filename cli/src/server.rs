use crate::{host_addr, options::Options};
use async_trait::async_trait;
use ouisync_bridge::{
    logger,
    protocol::{self, Request, Response},
    transport::{self, local::LocalServer},
    ClientState, Result, ServerState,
};
use ouisync_lib::StateMonitor;
use std::{io, sync::Arc};
use tokio::task;

#[derive(Clone)]
pub(crate) struct Handler {
    server_state: Arc<ServerState>,
}

impl Handler {
    pub fn new(server_state: Arc<ServerState>) -> Self {
        Self { server_state }
    }
}

#[async_trait]
impl transport::Handler for Handler {
    async fn handle(&self, client_state: &ClientState, request: Request) -> Result<Response> {
        protocol::dispatch(&self.server_state, client_state, request).await
    }
}

pub(crate) async fn run(options: Options) -> anyhow::Result<()> {
    let root_monitor = StateMonitor::make_root();
    let _logger = logger::new(root_monitor.clone());

    let state = ServerState::new(options.config_dir.into(), root_monitor);
    let state = Arc::new(state);

    let server = LocalServer::bind(host_addr::default_local())?;
    task::spawn(server.run(Handler::new(state.clone())));

    terminated().await?;
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

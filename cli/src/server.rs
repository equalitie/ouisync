use crate::{
    handler::{local::LocalHandler, remote::RemoteHandler},
    options::Dirs,
    state::State,
    transport::local::LocalServer,
};
use anyhow::Result;
use ouisync_bridge::{
    config::{ConfigError, ConfigKey},
    error::Error,
    logger,
    transport::RemoteServer,
};
use ouisync_lib::StateMonitor;
use scoped_task::ScopedAbortHandle;
use std::{
    io,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::task;

pub(crate) async fn run(dirs: Dirs, socket: String) -> Result<()> {
    let monitor = StateMonitor::make_root();
    let _logger = logger::new(Some(monitor.clone()));

    let state = State::init(&dirs, monitor).await?;
    let server = LocalServer::bind(Path::new(&socket))?;
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

const BIND_RPC_KEY: ConfigKey<Vec<SocketAddr>> =
    ConfigKey::new("bind_rpc", "Addresses to bind the remote API to");

#[derive(Default)]
pub(crate) struct ServerContainer {
    handles: Mutex<Vec<ScopedAbortHandle>>,
}

impl ServerContainer {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn init(&self, state: Arc<State>) -> Result<(), Error> {
        let entry = state.config.entry(BIND_RPC_KEY);
        let addrs = match entry.get().await {
            Ok(addrs) => addrs,
            Err(ConfigError::NotFound) => Vec::new(),
            Err(error) => return Err(error.into()),
        };

        let (handles, _) = start(&addrs, state).await?;
        *self.handles.lock().unwrap() = handles;

        Ok(())
    }

    pub async fn set(
        &self,
        addrs: &[SocketAddr],
        state: Arc<State>,
    ) -> Result<Vec<SocketAddr>, Error> {
        let entry = state.config.entry(BIND_RPC_KEY);
        let (handles, local_addrs) = start(addrs, state).await?;
        *self.handles.lock().unwrap() = handles;
        entry.set(&local_addrs).await?;

        Ok(local_addrs)
    }

    pub fn close(&self) {
        self.handles.lock().unwrap().clear();
    }
}

async fn start(
    addrs: &[SocketAddr],
    state: Arc<State>,
) -> Result<(Vec<ScopedAbortHandle>, Vec<SocketAddr>), Error> {
    let mut handles = Vec::with_capacity(addrs.len());
    let mut local_addrs = Vec::with_capacity(addrs.len());

    // Avoid loading the TLS config if not needed
    if addrs.is_empty() {
        return Ok((handles, local_addrs));
    }

    let config = state.get_tls_server_config().await?;

    for addr in addrs {
        let Ok(server) = RemoteServer::bind(*addr, config.clone()).await else {
            continue;
        };

        local_addrs.push(server.local_addr());

        handles.push(
            task::spawn(server.run(RemoteHandler::new(state.clone())))
                .abort_handle()
                .into(),
        );
    }

    Ok((handles, local_addrs))
}

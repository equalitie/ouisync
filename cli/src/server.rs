use crate::{
    handler::{local::LocalHandler, remote::RemoteHandler},
    options::Dirs,
    state::State,
    transport::local::LocalServer,
};
use anyhow::Result;
use ouisync_bridge::{config::ConfigKey, logger, transport::RemoteServer};
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

    let state = State::init(&dirs, monitor).await;
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

    pub async fn init(&self, state: Arc<State>) {
        let entry = state.config.entry(BIND_RPC_KEY);
        let addrs = match entry.get().await {
            Ok(addrs) => addrs,
            Err(error) => {
                tracing::error!(?error, "failed to get bind_rpc config");
                return;
            }
        };

        let (handles, _) = start(&addrs, state).await;
        *self.handles.lock().unwrap() = handles;
    }

    pub async fn set(&self, addrs: &[SocketAddr], state: Arc<State>) -> Vec<SocketAddr> {
        let entry = state.config.entry(BIND_RPC_KEY);
        let (handles, local_addrs) = start(addrs, state).await;
        *self.handles.lock().unwrap() = handles;

        if let Err(error) = entry.set(&local_addrs).await {
            tracing::error!(?error, "failed to set bind_rpc config");
        }

        local_addrs
    }

    pub fn close(&self) {
        self.handles.lock().unwrap().clear();
    }
}

async fn start(
    addrs: &[SocketAddr],
    state: Arc<State>,
) -> (Vec<ScopedAbortHandle>, Vec<SocketAddr>) {
    let mut handles = Vec::with_capacity(addrs.len());
    let mut local_addrs = Vec::with_capacity(addrs.len());

    for addr in addrs {
        let Ok(server) = RemoteServer::bind(*addr).await else {
                continue;
            };

        local_addrs.push(server.local_addr());
        handles.push(
            task::spawn(server.run(RemoteHandler::new(state.clone())))
                .abort_handle()
                .into(),
        );
    }

    (handles, local_addrs)
}

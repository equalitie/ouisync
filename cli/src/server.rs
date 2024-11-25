use crate::{defaults, options::ServerCommand};
use ouisync_bridge::logger::Logger;
use ouisync_service::{Error, Service};
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

    let mut service = Service::init(
        socket,
        config_dir,
        defaults::store_dir(),
        defaults::mount_dir(),
    )
    .await?;

    select! {
        result = service.run() => result?,
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

/*
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

        let (handles, _) = start(state, &addrs).await?;
        *self.handles.lock().unwrap() = handles;

        Ok(())
    }

    pub async fn set(
        &self,
        state: Arc<State>,
        addrs: &[SocketAddr],
    ) -> Result<Vec<SocketAddr>, Error> {
        let entry = state.config.entry(BIND_RPC_KEY);

        let (handles, addrs) = start(state, addrs).await?;
        *self.handles.lock().unwrap() = handles;
        entry.set(&addrs).await?;
        Ok(addrs)
    }

    pub fn close(&self) {
        self.handles.lock().unwrap().clear();
    }
}

async fn start(
    state: Arc<State>,
    addrs: &[SocketAddr],
) -> Result<(Vec<ScopedAbortHandle>, Vec<SocketAddr>), Error> {
    let mut handles = Vec::with_capacity(addrs.len());
    let mut local_addrs = Vec::with_capacity(addrs.len());

    // Avoid loading the TLS config if not needed
    if addrs.is_empty() {
        return Ok((handles, local_addrs));
    }

    let config = state.get_server_config().await?;

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
*/

use crate::{dart::PortSender, state::State, transport::ClientSender, utils::UniqueHandle};
use ouisync_bridge::logger::{LogFormat, Logger};
use ouisync_lib::StateMonitor;
use std::{io, path::PathBuf, str::Utf8Error, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{runtime, time};

pub struct Session {
    pub(crate) runtime: runtime::Runtime,
    pub(crate) state: Arc<State>,
    pub(crate) client_sender: ClientSender,
    pub(crate) port_sender: PortSender,
    _logger: Logger,
}

impl Session {
    pub(crate) fn create(
        configs_path: PathBuf,
        log_path: Option<PathBuf>,
        port_sender: PortSender,
        client_sender: ClientSender,
    ) -> Result<Self, SessionError> {
        let root_monitor = StateMonitor::make_root();

        // Init logger
        let logger = Logger::new(
            log_path.as_deref(),
            Some(root_monitor.clone()),
            LogFormat::Human,
        )
        .map_err(SessionError::InitializeLogger)?;

        // Create runtime
        let runtime = runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(SessionError::InitializeRuntime)?;
        let _enter = runtime.enter(); // runtime context is needed for some of the following calls

        let state = Arc::new(State::new(configs_path, root_monitor));
        let session = Session {
            runtime,
            state,
            client_sender,
            port_sender,
            _logger: logger,
        };

        Ok(session)
    }

    pub(crate) fn shutdown_network_and_close(self) {
        let Self {
            runtime,
            state,
            _logger,
            ..
        } = self;

        runtime.block_on(async move {
            time::timeout(Duration::from_millis(500), state.network.shutdown())
                .await
                .unwrap_or(())
        });
    }
}

pub type SessionHandle = UniqueHandle<Session>;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to initialize logger")]
    InitializeLogger(#[source] io::Error),
    #[error("failed to initialize runtime")]
    InitializeRuntime(#[source] io::Error),
    #[error("invalid utf8 string")]
    InvalidUtf8(#[from] Utf8Error),
}

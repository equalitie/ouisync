use crate::{
    error::{ErrorCode, ToErrorCode},
    handler::Handler,
    sender::Sender,
    state::State,
    transport::{ClientSender, Server},
    utils::{self, UniqueHandle},
};
use ouisync_bridge::logger::{LogFormat, Logger};
use ouisync_lib::StateMonitor;
use std::{ffi::c_char, io, path::PathBuf, ptr, str::Utf8Error, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{runtime, time};

pub struct Session {
    pub(crate) runtime: runtime::Runtime,
    pub(crate) state: Arc<State>,
    pub(crate) client_sender: ClientSender,
    _logger: Logger,
}

impl Session {
    pub(crate) fn create(
        configs_path: PathBuf,
        log_path: Option<PathBuf>,
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

#[repr(C)]
pub struct SessionCreateResult {
    session: SessionHandle,
    error_code: ErrorCode,
    error_message: *const c_char,
}

impl From<Result<Session, SessionError>> for SessionCreateResult {
    fn from(result: Result<Session, SessionError>) -> Self {
        match result {
            Ok(session) => Self {
                session: SessionHandle::new(Box::new(session)),
                error_code: ErrorCode::Ok,
                error_message: ptr::null(),
            },
            Err(error) => Self {
                session: SessionHandle::NULL,
                error_code: error.to_error_code(),
                error_message: utils::str_to_ptr(&error.to_string()),
            },
        }
    }
}

/// Helper for creating guest-language specific FFI wrappers
pub(crate) unsafe fn create(
    configs_path: *const c_char,
    log_path: *const c_char,
    sender: impl Sender,
) -> SessionCreateResult {
    let configs_path = match utils::ptr_to_str(configs_path) {
        Ok(configs_path) => PathBuf::from(configs_path),
        Err(error) => return Err(SessionError::from(error)).into(),
    };

    let log_path = match utils::ptr_to_maybe_str(log_path) {
        Ok(log_path) => log_path.map(PathBuf::from),
        Err(error) => return Err(SessionError::from(error)).into(),
    };

    let (server, client_tx) = Server::new(sender);
    let result = Session::create(configs_path, log_path, client_tx);

    if let Ok(session) = &result {
        session
            .runtime
            .spawn(server.run(Handler::new(session.state.clone())));
    }

    result.into()
}

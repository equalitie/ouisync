use crate::{
    error::{ErrorCode, ToErrorCode},
    handler::Handler,
    sender::Sender,
    state::State,
    transport::{ClientSender, Server},
    utils,
};
use ouisync_bridge::logger::{LogColor, LogFormat, Logger};
use state_monitor::StateMonitor;
use std::{
    ffi::c_char,
    io,
    marker::PhantomData,
    path::Path,
    ptr,
    str::Utf8Error,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use thiserror::Error;
use tokio::{runtime, time};

pub struct Session {
    pub(crate) shared: Arc<Shared>,
    pub(crate) client_tx: ClientSender,
}

/// State shared between multiple instances of the same session.
pub(crate) struct Shared {
    pub(crate) runtime: runtime::Runtime,
    pub(crate) state: Arc<State>,
    _logger: Logger,
}

impl Shared {
    fn new(configs_path: &Path, log_path: Option<&Path>) -> Result<Arc<Self>, SessionError> {
        let root_monitor = StateMonitor::make_root();

        // Init logger
        let logger = Logger::new(
            log_path,
            Some(root_monitor.clone()),
            LogFormat::Human,
            LogColor::Auto,
        )
        .map_err(SessionError::InitializeLogger)?;

        // Create runtime
        let runtime = runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(SessionError::InitializeRuntime)?;
        let _enter = runtime.enter(); // runtime context is needed for some of the following calls

        let state = Arc::new(State::new(configs_path.to_owned(), root_monitor));

        Ok(Arc::new(Self {
            runtime,
            state,
            _logger: logger,
        }))
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

/// What type of session to create.
///
/// `Shared` should be used by default. `Unique` is useful mostly for tests, to ensure test
/// isolation and/or to simulate multiple replicas in a single test.
#[repr(u8)]
pub enum SessionKind {
    /// Returns the global `Session` instance, creating it if not exists.
    Shared = 0,
    /// Always creates a new `Session` instance.
    Unique = 1,
}

/// Handle to [Session] which can be passed across the FFI boundary.
#[repr(transparent)]
pub struct SessionHandle(u64, PhantomData<Box<Session>>);

impl SessionHandle {
    pub const NULL: Self = Self(0, PhantomData);

    pub(crate) unsafe fn get(&self) -> &Session {
        assert_ne!(self.0, 0, "invalid handle");
        &*(self.0 as *const _)
    }

    pub(crate) unsafe fn release(self) -> Session {
        *Box::from_raw(self.0 as *mut _)
    }
}

impl From<Session> for SessionHandle {
    fn from(session: Session) -> Self {
        Self(Box::into_raw(Box::new(session)) as _, PhantomData)
    }
}

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
                session: SessionHandle::from(session),
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

static SHARED: Mutex<Weak<Shared>> = Mutex::new(Weak::new());

pub(crate) unsafe fn create(
    kind: SessionKind,
    configs_path: *const c_char,
    log_path: *const c_char,
    sender: impl Sender,
) -> Result<Session, SessionError> {
    let configs_path = Path::new(utils::ptr_to_str(configs_path)?);
    let log_path = utils::ptr_to_maybe_str(log_path)?.map(Path::new);

    let shared = match kind {
        SessionKind::Unique => Shared::new(configs_path, log_path)?,
        SessionKind::Shared => {
            let mut guard = SHARED.lock().unwrap();

            if let Some(shared) = guard.upgrade() {
                shared
            } else {
                let shared = Shared::new(configs_path, log_path)?;
                *guard = Arc::downgrade(&shared);
                shared
            }
        }
    };

    let (server, client_tx) = Server::new(sender);

    shared
        .runtime
        .spawn(server.run(Handler::new(shared.state.clone())));

    Ok(Session { shared, client_tx })
}

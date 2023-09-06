//! Dart FFI (foreign function interface) for the ouisync library.

#[macro_use]
mod utils;
mod constants;
mod dart;
mod directory;
mod error;
mod file;
mod handler;
mod network;
mod protocol;
mod registry;
mod repository;
mod share_token;
mod state;
mod state_monitor;
mod transport;

pub use constants::{
    ACCESS_MODE_BLIND, ACCESS_MODE_READ, ACCESS_MODE_WRITE, ENTRY_TYPE_DIRECTORY, ENTRY_TYPE_FILE,
};

use crate::{
    dart::{Port, PortSender},
    error::{ErrorCode, ToErrorCode},
    handler::Handler,
    state::State,
    transport::{ClientSender, Server},
    utils::UniqueHandle,
};
#[cfg(unix)]
use crate::{file::FileHolder, registry::Handle};
use bytes::Bytes;
use ouisync_bridge::logger::{LogFormat, Logger};
use ouisync_lib::StateMonitor;
use ouisync_vfs::MountError;
#[cfg(unix)]
use std::os::raw::c_int;
use std::{
    ffi::CString,
    io, mem,
    os::raw::{c_char, c_void},
    path::PathBuf,
    ptr, slice,
    str::Utf8Error,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    runtime::{self, Runtime},
    time,
};

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

/// Creates a ouisync session. `post_c_object_fn` should be a pointer to the dart's
/// `NativeApi.postCObject` function cast to `Pointer<Void>` (the casting is necessary to work
/// around limitations of the binding generators).
///
/// # Safety
///
/// - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
/// - `configs_path` must be a pointer to a nul-terminated utf-8 encoded string
#[no_mangle]
pub unsafe extern "C" fn session_create(
    post_c_object_fn: *const c_void,
    configs_path: *const c_char,
    log_path: *const c_char,
    server_tx_port: Port<Bytes>,
) -> SessionCreateResult {
    let port_sender = PortSender::new(mem::transmute(post_c_object_fn));

    let configs_path = match utils::ptr_to_str(configs_path) {
        Ok(configs_path) => PathBuf::from(configs_path),
        Err(error) => return Err(SessionError::from(error)).into(),
    };

    let log_path = match utils::ptr_to_maybe_str(log_path) {
        Ok(log_path) => log_path.map(PathBuf::from),
        Err(error) => return Err(SessionError::from(error)).into(),
    };

    let (server, client_tx) = Server::new(port_sender, server_tx_port);
    let result = Session::create(configs_path, log_path, port_sender, client_tx);

    if let Ok(session) = &result {
        session
            .runtime
            .spawn(server.run(Handler::new(session.state.clone())));
    }

    result.into()
}

/// Destroys the ouisync session.
///
/// # Safety
///
/// `session` must be a valid session handle.
#[no_mangle]
pub unsafe extern "C" fn session_destroy(session: SessionHandle) {
    session.release();
}

/// Mount all repositories that are or will be opened in read and/or write mode.
///
/// # Safety
///
/// `session` must be a valid session handle.
#[no_mangle]
pub unsafe extern "C" fn session_mount_all(
    session: SessionHandle,
    mount_point: *const c_char,
    port: Port<Result<(), MountError>>,
) {
    let session = session.get();

    let mount_point = match utils::ptr_to_str(mount_point) {
        Ok(mount_point) => PathBuf::from(mount_point),
        Err(_error) => {
            session
                .port_sender
                .send_result(port, Err(MountError::FailedToParseMountPoint));
            return;
        }
    };

    session.mount_all(mount_point, port)
}

/// # Safety
///
/// `session` must be a valid session handle, `sender` must be a valid client sender handle,
/// `payload_ptr` must be a pointer to a byte buffer whose length is at least `payload_len` bytes.
///
#[no_mangle]
pub unsafe extern "C" fn session_channel_send(
    session: SessionHandle,
    payload_ptr: *mut u8,
    payload_len: u64,
) {
    let payload = slice::from_raw_parts(payload_ptr, payload_len as usize);
    let payload = payload.into();

    session.get().client_sender.send(payload).ok();
}

/// Shutdowns the network and closes the session. This is equivalent to doing it in two steps
/// (`network_shutdown` then `session_close`), but in flutter when the engine is being detached
/// from Android runtime then async wait for `network_shutdown` never completes (or does so
/// randomly), and thus `session_close` is never invoked. My guess is that because the dart engine
/// is being detached we can't do any async await on the dart side anymore, and thus need to do it
/// here.
///
/// # Safety
///
/// `session` must be a valid session handle.
#[no_mangle]
pub unsafe extern "C" fn session_shutdown_network_and_close(session: SessionHandle) {
    let Session {
        runtime,
        state,
        _logger,
        ..
    } = *session.release();

    runtime.block_on(async move {
        time::timeout(Duration::from_millis(500), state.network.shutdown())
            .await
            .unwrap_or(())
    });
}

/// Copy the file contents into the provided raw file descriptor.
///
/// This function takes ownership of the file descriptor and closes it when it finishes. If the
/// caller needs to access the descriptor afterwards (or while the function is running), he/she
/// needs to `dup` it before passing it into this function.
///
/// # Safety
///
/// `session` must be a valid session handle, `handle` must be a valid file holder handle, `fd`
/// must be a valid and open file descriptor and `port` must be a valid dart native port.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn file_copy_to_raw_fd(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    fd: c_int,
    port: Port<Result<(), ouisync_lib::Error>>,
) {
    use std::os::unix::io::FromRawFd;
    use tokio::fs;

    let session = session.get();
    let port_sender = session.port_sender;

    let src = session.state.files.get(handle);
    let mut dst = fs::File::from_raw_fd(fd);

    session.runtime.spawn(async move {
        let mut src = src.file.lock().await;
        let result = src.copy_to_writer(&mut dst).await;

        port_sender.send_result(port, result);
    });
}

/// Deallocate string that has been allocated on the rust side
///
/// # Safety
///
/// `ptr` must be a pointer obtained from a call to `CString::into_raw`.
#[no_mangle]
pub unsafe extern "C" fn free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }

    let _ = CString::from_raw(ptr);
}

/// Print log message
///
/// # Safety
///
/// `message_ptr` must be a pointer to a nul-terminated utf-8 encoded string
#[no_mangle]
pub unsafe extern "C" fn log_print(
    level: u8,
    scope_ptr: *const c_char,
    message_ptr: *const c_char,
) {
    let scope = match utils::ptr_to_str(scope_ptr) {
        Ok(scope) => scope,
        Err(error) => {
            tracing::error!(?error, "invalid log scope string");
            return;
        }
    };

    let _enter = tracing::info_span!("app", scope).entered();

    let message = match utils::ptr_to_str(message_ptr) {
        Ok(message) => message,
        Err(error) => {
            tracing::error!(?error, "invalid log message string");
            return;
        }
    };

    match level {
        LOG_LEVEL_ERROR => tracing::error!("{}", message),
        LOG_LEVEL_WARN => tracing::warn!("{}", message),
        LOG_LEVEL_INFO => tracing::info!("{}", message),
        LOG_LEVEL_DEBUG => tracing::debug!("{}", message),
        LOG_LEVEL_TRACE => tracing::trace!("{}", message),
        _ => {
            tracing::error!(level, "invalid log level");
        }
    }
}

pub const LOG_LEVEL_ERROR: u8 = 1;
pub const LOG_LEVEL_WARN: u8 = 2;
pub const LOG_LEVEL_INFO: u8 = 3;
pub const LOG_LEVEL_DEBUG: u8 = 4;
pub const LOG_LEVEL_TRACE: u8 = 5;

pub struct Session {
    pub(crate) runtime: Runtime,
    pub(crate) state: Arc<State>,
    client_sender: ClientSender,
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

    // TODO: Linux, OSX
    #[cfg(not(target_os = "windows"))]
    fn mount_all(&self, _mount_point: PathBuf, port: Port<Result<(), MountError>>) {
        self.port_sender
            .send_result(port, Err(MountError::UnsupportedOs))
    }

    #[cfg(target_os = "windows")]
    fn mount_all(&self, mount_point: PathBuf, port: Port<Result<(), MountError>>) {
        let state = self.state.clone();
        let runtime = self.runtime.handle().clone();
        let port_sender = self.port_sender;

        self.runtime.spawn(async move {
            use ouisync_vfs::MultiRepoVFS;

            // TODO: Let the user chose what the mount point is.
            let mounter = match MultiRepoVFS::mount(runtime, mount_point).await {
                Ok(mounter) => mounter,
                Err(error) => {
                    tracing::error!("Failed to mount session: {error:?}");
                    port_sender.send_result(port, Err(error));
                    return;
                }
            };

            let repos = state.read_repositories();

            for repo_holder in repos.values() {
                if let Err(error) = mounter.add_repo(
                    repo_holder.store_path.clone(),
                    repo_holder.repository.clone(),
                ) {
                    tracing::error!(
                        "Failed to mount repository {:?}: {error:?}",
                        repo_holder.store_path
                    );
                }
            }

            *(state.mounter.lock().unwrap()) = Some(mounter);

            port_sender.send_result(port, Ok(()));
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

//! Dart FFI (foreign function interface) for the ouisync library.

#[macro_use]
mod utils;
mod dart;

use crate::{
    dart::{DartCObject, PostDartCObjectFn},
    utils::{Port, UniqueHandle},
};
use ouisync_bridge::{
    logger::{self, Logger},
    run_client,
    socket::{self, memory::ClientSender},
    Error, ErrorCode, FileHolder, Handle, Registry, Result, ServerState,
};
use ouisync_lib::StateMonitor;
use std::{
    ffi::CString,
    mem,
    os::raw::{c_char, c_int, c_void},
    path::PathBuf,
    ptr, slice,
    sync::Arc,
    time::Duration,
};
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

impl From<Result<Session>> for SessionCreateResult {
    fn from(result: Result<Session>) -> Self {
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
) -> SessionCreateResult {
    let port_sender = PortSender {
        post_c_object_fn: mem::transmute(post_c_object_fn),
    };

    let configs_path = match utils::ptr_to_str(configs_path) {
        Ok(configs_path) => PathBuf::from(configs_path),
        Err(error) => return Err(error).into(),
    };

    Session::create(port_sender, configs_path).into()
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

/// Create in-memory interface channel for when the client and the server are both in the same
/// process.
///
/// # Safety
///
/// `session` must be a valid session handle. `port` must be a valid dart native port.
#[no_mangle]
pub unsafe extern "C" fn session_channel_open(
    session: SessionHandle,
    port: Port<Vec<u8>>,
) -> Handle<ClientSender> {
    let session = session.get();
    let state = session.state.clone();
    let port_sender = session.port_sender;

    let (server_stream, client_tx, mut client_rx) = socket::memory::new();

    session
        .runtime
        .spawn(async move { run_client(server_stream, &state).await });

    session.runtime.spawn(async move {
        while let Some(payload) = client_rx.recv().await {
            port_sender.send(port, payload);
        }
    });

    session.senders.insert(client_tx)
}

/// # Safety
///
/// `session` must be a valid session handle, `sender` must be a valid client sender handle,
/// `payload_ptr` must be a pointer to a byte buffer whose length is at least `payload_len` bytes.
///
#[no_mangle]
pub unsafe extern "C" fn session_channel_send(
    session: SessionHandle,
    sender: Handle<ClientSender>,
    payload_ptr: *mut u8,
    payload_len: u64,
) {
    let payload = slice::from_raw_parts(payload_ptr, payload_len as usize);
    let payload = payload.into();

    session.get().senders.get(sender).send(payload).ok();
}

/// # Safety
///
/// `session` must be a valid session handle and `sender` must be a valid client sender handle.
#[no_mangle]
pub unsafe extern "C" fn session_channel_close(
    session: SessionHandle,
    sender: Handle<ClientSender>,
) {
    session.get().senders.remove(sender);
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
        time::timeout(
            Duration::from_millis(500),
            state.network.handle().shutdown(),
        )
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
    port: Port<Result<()>>,
) {
    use std::os::unix::io::FromRawFd;
    use tokio::fs;

    let session = session.get();
    let port_sender = session.port_sender;

    let src = session.state.files.get(handle);
    let mut dst = fs::File::from_raw_fd(fd);

    session.runtime.spawn(async move {
        let mut src = src.file.lock().await;
        let result = src.copy_to_writer(&mut dst).await.map_err(Error::from);

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

pub struct Session {
    pub(crate) runtime: Runtime,
    pub(crate) state: Arc<ServerState>,
    senders: Registry<ClientSender>,
    pub(crate) port_sender: PortSender,
    _logger: Logger,
}

impl Session {
    pub(crate) fn create(port_sender: PortSender, configs_path: PathBuf) -> Result<Self> {
        let root_monitor = StateMonitor::make_root();

        // Init logger
        let logger = logger::new(root_monitor.clone())?;

        // Create runtime
        let runtime = runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(Error::InitializeRuntime)?;
        let _enter = runtime.enter(); // runtime context is needed for some of the following calls

        let state = Arc::new(ServerState::new(configs_path, root_monitor)?);
        let session = Session {
            runtime,
            state,
            senders: Registry::new(),
            port_sender,
            _logger: logger,
        };

        Ok(session)
    }
}

pub type SessionHandle = UniqueHandle<Session>;

// Utility for sending values to dart.
#[derive(Copy, Clone)]
struct PortSender {
    post_c_object_fn: PostDartCObjectFn,
}

impl PortSender {
    unsafe fn send<T>(&self, port: Port<T>, value: T)
    where
        T: Into<DartCObject>,
    {
        (self.post_c_object_fn)(port.into(), &mut value.into());
    }

    unsafe fn send_result<T>(&self, port: Port<Result<T>>, value: Result<T>)
    where
        T: Into<DartCObject>,
    {
        let port = port.into();

        match value {
            Ok(value) => {
                (self.post_c_object_fn)(port, &mut ErrorCode::Ok.into());
                (self.post_c_object_fn)(port, &mut value.into());
            }
            Err(error) => {
                tracing::error!("ffi error: {:?}", error);
                (self.post_c_object_fn)(port, &mut error.to_error_code().into());
                (self.post_c_object_fn)(port, &mut error.to_string().into());
            }
        }
    }
}

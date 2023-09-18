//! Dart FFI (foreign function interface) for the ouisync library.

#[macro_use]
mod utils;
mod dart;
mod directory;
mod error;
mod file;
mod handler;
mod network;
mod protocol;
mod registry;
mod repository;
mod session;
mod share_token;
mod state;
mod state_monitor;
mod transport;

use crate::{
    dart::{Port, PortSender},
    error::{ErrorCode, ToErrorCode},
    file::FileHolder,
    handler::Handler,
    registry::Handle,
    session::SessionHandle,
    transport::Server,
};
use bytes::Bytes;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use session::{Session, SessionError};
use std::{
    ffi::CString,
    mem,
    os::raw::{c_char, c_int, c_void},
    path::PathBuf,
    ptr, slice,
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
    configs_path: *const c_char,
    log_path: *const c_char,
    post_c_object_fn: *const c_void,
    port: Port,
) -> SessionCreateResult {
    let sender = PortSender::new(mem::transmute(post_c_object_fn), port);

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

/// Closes the ouisync session.
///
/// # Safety
///
/// `session` must be a valid session handle.
#[no_mangle]
pub unsafe extern "C" fn session_close(session: SessionHandle) {
    session.release();
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
    session.release().shutdown_network_and_close();
}

/// Copy the file contents into the provided raw file descriptor.
///
/// This function takes ownership of the file descriptor and closes it when it finishes. If the
/// caller needs to access the descriptor afterwards (or while the function is running), he/she
/// needs to `dup` it before passing it into this function.
///
/// # Safety
///
/// - `session` must be a valid session handle
/// - `handle` must be a valid file holder handle
/// - `fd` must be a valid and open file descriptor
/// - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
/// - `port` must be a valid dart native port
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn file_copy_to_raw_fd(
    session: SessionHandle,
    handle: Handle<FileHolder>,
    fd: c_int,
    post_c_object_fn: *const c_void,
    port: Port,
) {
    use crate::{error::Error, session::Sender};
    use bytes::{BufMut, BytesMut};
    use std::{io::SeekFrom, os::fd::FromRawFd};
    use tokio::fs;

    let session = session.get();
    let sender = PortSender::new(mem::transmute(post_c_object_fn), port);

    let src = session.state.files.get(handle);
    let mut dst = fs::File::from_raw_fd(fd);

    session.runtime.spawn(async move {
        let mut src = src.file.lock().await;
        src.seek(SeekFrom::Start(0));
        let result = src.copy_to_writer(&mut dst).await;

        match result {
            Ok(()) => sender.send(Bytes::new()),
            Err(error) => sender.send(encode_error(&error.into())),
        }
    });

    fn encode_error(error: &Error) -> Bytes {
        let mut buffer = BytesMut::new();
        buffer.put_u16(error.code as u16);
        buffer.put_slice(error.message.as_bytes());
        buffer.freeze()
    }
}

/// Always returns `OperationNotSupported` error. Defined to avoid lookup errors on non-unix
/// platforms. Do not use.
///
/// # Safety
///
/// - `session` must be a valid session handle.
/// - `port` must be a valid dart native port.
/// - `handle` and `fd` are not actually used and so have no safety requirements.
#[cfg(not(unix))]
#[no_mangle]
pub unsafe extern "C" fn file_copy_to_raw_fd(
    session: SessionHandle,
    _handle: Handle<FileHolder>,
    _fd: c_int,
    port: Port<Result<(), ouisync_lib::Error>>,
) {
    session
        .get()
        .port_sender
        .send_result(port, Err(ouisync_lib::Error::OperationNotSupported))
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

    match level.try_into() {
        Ok(LogLevel::Error) => tracing::error!("{}", message),
        Ok(LogLevel::Warn) => tracing::warn!("{}", message),
        Ok(LogLevel::Info) => tracing::info!("{}", message),
        Ok(LogLevel::Debug) => tracing::debug!("{}", message),
        Ok(LogLevel::Trace) => tracing::trace!("{}", message),
        Err(_) => tracing::error!(level, "invalid log level"),
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, IntoPrimitive, TryFromPrimitive)]
#[repr(u8)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

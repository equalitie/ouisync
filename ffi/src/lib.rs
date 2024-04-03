//! Dart FFI (foreign function interface) for the ouisync library.

#[macro_use]
mod utils;
mod c;
mod dart;
mod directory;
mod error;
mod file;
mod handler;
mod log;
mod mounter;
mod network;
mod protocol;
mod registry;
mod repository;
mod sender;
mod session;
mod share_token;
mod state;
mod state_monitor;
mod transport;

use crate::{
    c::{Callback, CallbackSender},
    dart::{Port, PortSender, PostDartCObjectFn},
    error::Error,
    file::FileHandle,
    log::LogLevel,
    sender::Sender,
    session::{SessionCreateResult, SessionHandle},
};
use session::SessionKind;
use std::{
    ffi::CString,
    os::raw::{c_char, c_int},
    slice,
};

/// Creates a ouisync session (common C-like API)
///
/// # Safety
///
/// - `configs_path` and `log_path` must be pointers to nul-terminated utf-8 encoded strings.
/// - `context` must be a valid pointer to a value that outlives the `Session` and that is safe
///   to be sent to other threads or null.
/// - `callback` must be a valid function pointer which does not leak the passed `msg_ptr`.
#[no_mangle]
pub unsafe extern "C" fn session_create(
    kind: SessionKind,
    configs_path: *const c_char,
    log_path: *const c_char,
    context: *mut (),
    callback: Callback,
) -> SessionCreateResult {
    let sender = CallbackSender::new(context, callback);
    session::create(kind, configs_path, log_path, sender).into()
}

/// Creates a ouisync session (dart-specific API)
///
/// # Safety
///
/// - `configs_path` and `log_path` must be pointers to nul-terminated utf-8 encoded strings.
/// - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
#[no_mangle]
pub unsafe extern "C" fn session_create_dart(
    kind: SessionKind,
    configs_path: *const c_char,
    log_path: *const c_char,
    post_c_object_fn: PostDartCObjectFn,
    port: Port,
) -> SessionCreateResult {
    let sender = PortSender::new(post_c_object_fn, port);
    session::create(kind, configs_path, log_path, sender).into()
}

#[no_mangle]
pub unsafe extern "C" fn session_grab_shared(
    context: *mut (),
    callback: Callback,
) -> SessionCreateResult {
    let sender = CallbackSender::new(context, callback);
    session::grab_shared(sender).into()
}

/// Closes the Ouisync session (common C-like API).
///
/// Also gracefully disconnects from all peers and asynchronously waits for the disconnections to
/// complete.
///
/// # Safety
///
/// `session` must be a valid session handle.
/// `callback` must be a valid function pointer which does not leak the passed `msg_ptr`.
#[no_mangle]
pub unsafe extern "C" fn session_close(
    session: SessionHandle,
    context: *mut (),
    callback: Callback,
) {
    let sender = CallbackSender::new(context, callback);
    session::close(session.release(), sender)
}

/// Closes the Ouisync session (dart-specific API).
///
/// Also gracefully disconnects from all peers and asynchronously waits for the disconnections to
/// complete.
///
/// # Safety
///
/// - `session` must be a valid session handle.
/// - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
#[no_mangle]
pub unsafe extern "C" fn session_close_dart(
    session: SessionHandle,
    post_c_object_fn: PostDartCObjectFn,
    port: Port,
) {
    let sender = PortSender::new(post_c_object_fn, port);
    session::close(session.release(), sender)
}

/// Closes the Ouisync session synchronously.
///
/// This is similar to `session_close` / `session_close_dart` but it blocks while waiting for the
/// graceful disconnect (with a short timeout to not block indefinitely). This is useful because in
/// flutter when the engine is being detached from Android runtime then async wait never completes
/// (or does so randomly), and thus `session_close` is never invoked. My guess is that because the
/// dart engine is being detached we can't do any async await on the dart side anymore, and thus
/// need to do it here.
///
/// # Safety
///
/// `session` must be a valid session handle.
#[no_mangle]
pub unsafe extern "C" fn session_close_blocking(session: SessionHandle) {
    session::close_blocking(session.release());
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

    session.get().client_tx.send(payload).ok();
}

/// Copy the file contents into the provided raw file descriptor (dart-specific API).
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
pub unsafe extern "C" fn file_copy_to_raw_fd_dart(
    session: SessionHandle,
    handle: FileHandle,
    fd: c_int,
    post_c_object_fn: PostDartCObjectFn,
    port: Port,
) {
    use bytes::Bytes;
    use std::{io::SeekFrom, os::fd::FromRawFd};
    use tokio::fs;

    let session = session.get();
    let sender = PortSender::new(post_c_object_fn, port);

    let src = match session.shared.state.files.get(handle) {
        Ok(file) => file,
        Err(error) => {
            sender.send(encode_error(&error.into()));
            return;
        }
    };

    let mut dst = fs::File::from_raw_fd(fd);

    session.shared.runtime.spawn(async move {
        let mut src = src.file.lock().await;
        src.seek(SeekFrom::Start(0));
        let result = src.copy_to_writer(&mut dst).await;

        match result {
            Ok(()) => sender.send(Bytes::new()),
            Err(error) => sender.send(encode_error(&error.into())),
        }
    });
}

/// Always returns `OperationNotSupported` error. Defined to avoid lookup errors on non-unix
/// platforms. Do not use.
///
/// # Safety
///
/// - `post_c_object_fn` must be a pointer to the dart's `NativeApi.postCObject` function
/// - `port` must be a valid dart native port.
/// - `session`, `handle` and `fd` are not actually used and so have no safety requirements.
#[cfg(not(unix))]
#[no_mangle]
pub unsafe extern "C" fn file_copy_to_raw_fd_dart(
    _session: SessionHandle,
    _handle: FileHandle,
    _fd: c_int,
    post_c_object_fn: PostDartCObjectFn,
    port: Port,
) {
    let sender = PortSender::new(post_c_object_fn, port);
    sender.send(encode_error(
        &ouisync_lib::Error::OperationNotSupported.into(),
    ))
}

fn encode_error(error: &Error) -> bytes::Bytes {
    use bytes::{BufMut, BytesMut};

    let mut buffer = BytesMut::new();
    buffer.put_u16(error.code as u16);
    buffer.put_slice(error.message.as_bytes());
    buffer.freeze()
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

    // NOTE: Passing `scope` as `message` for more succinct span rendering: `app{"foo"}` instead
    // of `app{scope="foo"}`.
    let _enter = tracing::info_span!("app", message = scope).entered();

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

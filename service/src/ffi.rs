use std::{
    ffi::{c_char, c_void, CStr, CString},
    io,
    path::Path,
    pin::pin,
    sync::OnceLock,
    thread,
};

use tokio::{runtime, select, sync::oneshot};
use tracing::{Instrument, Span};

use self::callback::Callback;
use crate::{
    logger::{LogColor, LogFormat, Logger},
    protocol::{ErrorCode, LogLevel, ToErrorCode},
    Error, Service,
};

/// Start Ouisync service in a new thread and bind it to the specified local socket.
///
/// Invokes `callback` after the service initialization completes. The first argument is the
/// `callback_context` argument unchanged, the second argument is the error code indicating the
/// status of the service initialization. If this is `Ok`, the service has been initialized
/// successfully and is ready to accept client connections.
///
/// Returns an opaque handle which must be passed to [ouisync_stop] to terminate the service.
///
/// # Safety
///
/// - `socket_path` and `config_dir` must be safe to pass to [std::ffi::CStr::from_ptr].
/// - `debug_label` must be either null or must be safe to pass to [std::ffi::CStr::from_ptr].
/// - `callback_context` must be either null or it must be safe to access from multiple threads.
#[no_mangle]
pub unsafe extern "C" fn ouisync_start(
    socket_path: *const c_char,
    config_dir: *const c_char,
    debug_label: *const c_char,
    callback: extern "C" fn(*const c_void, ErrorCode),
    callback_context: *const c_void,
) -> *mut c_void {
    let socket_path = CStr::from_ptr(socket_path).to_owned();
    let config_dir = CStr::from_ptr(config_dir).to_owned();
    let debug_label = if !debug_label.is_null() {
        Some(CStr::from_ptr(debug_label).to_owned())
    } else {
        None
    };
    let callback = Callback::new(callback, callback_context);

    let (stop_tx, stop_rx) = oneshot::channel();
    let stop_tx = Box::into_raw(Box::new(stop_tx)) as _;

    thread::spawn(move || run(socket_path, config_dir, debug_label, callback, stop_rx));

    stop_tx
}

/// Stops a running Ouisync service.
///
/// Invokes `callback` after the service shutdown has been completed.
///
/// # Safety
///
/// - `handle must have been obtained by calling `ouisync_start` and it must not have already been
/// passed to `ouisync_stop`.
/// - `callback_context` must be either null of it must be safe to access from multiple threads.
#[no_mangle]
pub unsafe extern "C" fn ouisync_stop(
    handle: *mut c_void,
    callback: extern "C" fn(*const c_void, ErrorCode),
    callback_context: *const c_void,
) {
    let tx: oneshot::Sender<Callback> = *Box::from_raw(handle as _);
    tx.send(Callback::new(callback, callback_context)).ok();
}

mod callback {
    use std::ffi::c_void;

    use crate::protocol::ErrorCode;

    pub(super) struct Callback {
        f: extern "C" fn(*const c_void, ErrorCode),
        cx: *const c_void,
    }

    impl Callback {
        /// # Safety
        ///
        /// `cx` must be safe to access from multiple threads
        pub unsafe fn new(f: extern "C" fn(*const c_void, ErrorCode), cx: *const c_void) -> Self {
            Self { f, cx }
        }

        pub fn call(&self, error_code: ErrorCode) {
            (self.f)(self.cx, error_code);
        }
    }

    /// Safety: this is safe assuming the `Callback` is constructed using `new` and it's safety
    /// invariants are upheld.
    unsafe impl Send for Callback {}
}

fn run(
    socket_path: CString,
    config_dir: CString,
    debug_label: Option<CString>,
    on_init: Callback,
    on_stop_rx: oneshot::Receiver<Callback>,
) {
    let (runtime, mut service, span) = match init(socket_path, config_dir, debug_label) {
        Ok(parts) => {
            on_init.call(ErrorCode::Ok);
            parts
        }
        Err(error) => {
            on_init.call(error.to_error_code());
            return;
        }
    };

    runtime.block_on(
        async move {
            let mut on_stop_rx = pin!(on_stop_rx);
            let (run_result, on_stop_result) = select! {
                result = service.run() => {
                    match result {
                        Err(error) => (Err(error), on_stop_rx.await)
                    }
                }
                result = &mut on_stop_rx => {
                    (Ok(()), result)
                }
            };

            service.close().await;
            drop(service);

            if let Ok(on_stop) = on_stop_result {
                on_stop.call(run_result.to_error_code());
            }
        }
        .instrument(span),
    );
}

fn init(
    socket_path: CString,
    config_dir: CString,
    debug_label: Option<CString>,
) -> Result<(runtime::Runtime, Service, Span), Error> {
    let socket_path = socket_path.into_string()?.into();
    let config_dir = config_dir.into_string()?.into();

    let span = if let Some(debug_label) = debug_label {
        tracing::info_span!("service", message = debug_label.into_string()?)
    } else {
        tracing::info_span!("service")
    };

    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(Error::InitializeRuntime)?;

    let service =
        runtime.block_on(Service::init(socket_path, config_dir).instrument(span.clone()))?;
    service.enable_panic_monitor();

    Ok((runtime, service, span))
}

/// Initialize logging. Should be called before `ouisync_start`.
///
/// Logs using the platforms' default logging infrastructure. If `log_file` is not null,
/// additionally logs to that file.
///
/// # Safety
///
/// `log_file` must be either null or it must be safe to pass to [std::ffi::CStr::from_ptr].
/// `log_tag` must be non-null and safe to pass to [std::ffi::CStr::from_ptr].
#[no_mangle]
pub unsafe extern "C" fn ouisync_log_init(
    log_file: *const c_char,
    log_tag: *const c_char,
) -> ErrorCode {
    log_init(log_file, log_tag).to_error_code()
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

unsafe fn log_init(log_file: *const c_char, log_tag: *const c_char) -> Result<(), Error> {
    let log_file = if log_file.is_null() {
        None
    } else {
        Some(Path::new(CStr::from_ptr(log_file).to_str()?))
    };

    let log_tag = CStr::from_ptr(log_tag).to_str()?.to_owned();

    let logger = Logger::new(log_file, log_tag, LogFormat::Human, LogColor::Always)
        .map_err(Error::InitializeLogger)?;

    LOGGER.set(logger).map_err(|_| {
        Error::InitializeLogger(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "logger already initialized",
        ))
    })?;

    Ok(())
}

/// Print log message using the logger created with `ouisync_log_init`.
///
/// # Safety
///
/// `scope_ptr` and `message_ptr` must be safe to pass to [std::ffi::CStr::from_ptr].
#[no_mangle]
pub unsafe extern "C" fn ouisync_log_print(
    level: u8,
    scope_ptr: *const c_char,
    message_ptr: *const c_char,
) {
    let scope = match CStr::from_ptr(scope_ptr).to_str() {
        Ok(scope) => scope,
        Err(error) => {
            tracing::error!(?error, "invalid log scope string");
            return;
        }
    };

    // NOTE: Passing `scope` as `message` for more succinct span rendering: `app{"foo"}` instead
    // of `app{scope="foo"}`.
    let _enter = tracing::info_span!("app", message = scope).entered();

    let message = match CStr::from_ptr(message_ptr).to_str() {
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

#[cfg(test)]
mod tests {
    use std::{fs, ptr, sync::mpsc};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn sanity_check() {
        let temp_dir = TempDir::new().unwrap();

        let socket_path = temp_dir.path().join("sock");

        let config_dir = temp_dir.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();

        let socket_path =
            CString::new(socket_path.into_os_string().into_string().unwrap()).unwrap();
        let config_dir = CString::new(config_dir.into_os_string().into_string().unwrap()).unwrap();

        extern "C" fn callback(cx: *const c_void, error_code: ErrorCode) {
            let tx: Box<mpsc::Sender<_>> = unsafe { Box::from_raw(cx as _) };
            tx.send(error_code).unwrap();
        }

        let (tx, rx) = mpsc::channel::<ErrorCode>();
        let handle = unsafe {
            ouisync_start(
                socket_path.as_ptr(),
                config_dir.as_ptr(),
                ptr::null(),
                callback,
                Box::into_raw(Box::new(tx)) as _,
            )
        };

        assert_eq!(rx.recv().unwrap(), ErrorCode::Ok);

        let (tx, rx) = mpsc::channel::<ErrorCode>();
        unsafe {
            ouisync_stop(handle, callback, Box::into_raw(Box::new(tx)) as _);
        }

        assert_eq!(rx.recv().unwrap(), ErrorCode::Ok);
    }
}

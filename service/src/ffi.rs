use std::{
    ffi::{c_char, c_uchar, c_ulong, c_void, CStr, CString},
    io, mem,
    path::Path,
    pin::pin,
    sync::OnceLock,
    thread,
};

use tokio::{runtime, select, sync::oneshot};
use tracing::{Instrument, Span};

use self::callback::Callback;
use crate::{
    logger::{BufferPool, Logger},
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
/// - `config_dir` must be safe to pass to [std::ffi::CStr::from_ptr].
/// - `debug_label` must be either null or must be safe to pass to [std::ffi::CStr::from_ptr].
/// - `callback_context` must be either null or it must be safe to access from multiple threads.
#[no_mangle]
pub unsafe extern "C" fn start_service(
    config_dir: *const c_char,
    debug_label: *const c_char,
    callback: extern "C" fn(*const c_void, ErrorCode),
    callback_context: *const c_void,
) -> *mut c_void {
    let config_dir = CStr::from_ptr(config_dir).to_owned();
    let debug_label = if !debug_label.is_null() {
        Some(CStr::from_ptr(debug_label).to_owned())
    } else {
        None
    };
    let callback = Callback::new(callback, callback_context);

    let (stop_tx, stop_rx) = oneshot::channel();
    let stop_tx = Box::into_raw(Box::new(stop_tx)) as _;

    thread::spawn(move || run(config_dir, debug_label, callback, stop_rx));

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
pub unsafe extern "C" fn stop_service(
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
    config_dir: CString,
    debug_label: Option<CString>,
    on_init: Callback,
    on_stop_rx: oneshot::Receiver<Callback>,
) {
    let (runtime, mut service, span) = match init(config_dir, debug_label) {
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
    config_dir: CString,
    debug_label: Option<CString>,
) -> Result<(runtime::Runtime, Service, Span), Error> {
    let config_dir = config_dir.into_string()?.into();

    let span = if let Some(debug_label) = debug_label {
        tracing::info_span!("service", message = debug_label.into_string()?)
    } else {
        Span::none()
    };

    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(Error::InitializeRuntime)?;

    let service = runtime.block_on(Service::init(config_dir).instrument(span.clone()))?;
    service.enable_panic_monitor();

    Ok((runtime, service, span))
}

// hoist Option here as a workaround for https://github.com/mozilla/cbindgen/issues/326
pub type OptionLogCallback = Option<extern "C" fn(LogLevel, *const c_uchar, c_ulong, c_ulong) -> ()>;

/// Initialize logging. Should be called before `service_start`.
///
/// If `file` is not null, write log messages to the given file.
///
/// If `callback` is not null, it is invoked for each log message. After the log message has been
/// processed, it needs to be released by calling `release_log_message`. Failure to do so will
/// cause memory leak. The messages can be processed asynchronously (e.g., in another thread).
///
/// # Safety
///
/// `file` must be either null or it must be safe to pass to [std::ffi::CStr::from_ptr].
#[no_mangle]
pub unsafe extern "C" fn init_log(file: *const c_char, callback: OptionLogCallback) -> ErrorCode {
    try_init_log(file, callback).to_error_code()
}

/// Release a log message back to the backend. See `init_log` for more details.
///
/// # Safety
///
/// `ptr`, `len` and `cap` must have been obtained through the callback to `init_log` and not
/// modified.
#[no_mangle]
pub unsafe extern "C" fn release_log_message(ptr: *const c_uchar, len: c_ulong, cap: c_ulong) {
    let message = Vec::from_raw_parts(ptr as _, len as _, cap as _);

    if let Some(pool) = LOGGER.get().and_then(|wrapper| wrapper.pool.as_ref()) {
        pool.release(message);
    }
}

struct LoggerWrapper {
    _logger: Logger,
    pool: Option<BufferPool>,
}

static LOGGER: OnceLock<LoggerWrapper> = OnceLock::new();

unsafe fn try_init_log(file: *const c_char, callback: OptionLogCallback) -> Result<(), Error> {
    let builder = Logger::builder();
    let builder = if !file.is_null() {
        builder.file(Path::new(CStr::from_ptr(file).to_str()?))
    } else {
        builder
    };

    let (builder, pool) = if let Some(callback) = callback {
        let pool = BufferPool::default();
        let callback = Box::new(move |level, message: &mut Vec<u8>| {
            let message = mem::take(message);
            let ptr = message.as_ptr();
            let len = message.len();
            let cap = message.capacity();
            mem::forget(message);

            callback(LogLevel::from(level), ptr, len as _, cap as _);
        });

        (builder.callback(callback, pool.clone()), Some(pool))
    } else {
        (builder, None)
    };

    let logger = builder.build().map_err(Error::InitializeLogger)?;

    LOGGER
        .set(LoggerWrapper {
            _logger: logger,
            pool,
        })
        .map_err(|_| {
            Error::InitializeLogger(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "logger already initialized",
            ))
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, ptr, sync::mpsc};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn sanity_check() {
        let temp_dir = TempDir::new().unwrap();

        let config_dir = temp_dir.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();

        let config_dir = CString::new(config_dir.into_os_string().into_string().unwrap()).unwrap();

        extern "C" fn callback(cx: *const c_void, error_code: ErrorCode) {
            let tx: Box<mpsc::Sender<_>> = unsafe { Box::from_raw(cx as _) };
            tx.send(error_code).unwrap();
        }

        let (tx, rx) = mpsc::channel::<ErrorCode>();
        let handle = unsafe {
            start_service(
                config_dir.as_ptr(),
                ptr::null(),
                callback,
                Box::into_raw(Box::new(tx)) as _,
            )
        };

        assert_eq!(rx.recv().unwrap(), ErrorCode::Ok);

        let (tx, rx) = mpsc::channel::<ErrorCode>();
        unsafe {
            stop_service(handle, callback, Box::into_raw(Box::new(tx)) as _);
        }

        assert_eq!(rx.recv().unwrap(), ErrorCode::Ok);
    }
}

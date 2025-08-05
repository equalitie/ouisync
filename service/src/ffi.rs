use std::{
    ffi::{c_char, c_void, CStr, CString},
    pin::pin,
    thread,
};

use tokio::{runtime, select, sync::oneshot};
use tracing::{Instrument, Span};

use self::callback::Callback;
use crate::{
    logger::{self, LogColor, LogFormat},
    protocol::{ErrorCode, ToErrorCode},
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn start_service(
    config_dir: *const c_char,
    debug_label: *const c_char,
    callback: extern "C" fn(*const c_void, ErrorCode),
    callback_context: *const c_void,
) -> *mut c_void {
    let config_dir = unsafe { CStr::from_ptr(config_dir) }.to_owned();
    let debug_label = if !debug_label.is_null() {
        Some(unsafe { CStr::from_ptr(debug_label) }.to_owned())
    } else {
        None
    };
    let callback = unsafe { Callback::new(callback, callback_context) };

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn stop_service(
    handle: *mut c_void,
    callback: extern "C" fn(*const c_void, ErrorCode),
    callback_context: *const c_void,
) {
    let tx: oneshot::Sender<Callback> = unsafe { *Box::from_raw(handle as _) };
    let cb = unsafe { Callback::new(callback, callback_context) };
    tx.send(cb).ok();
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
    // Get config dir
    let config_dir = match config_dir.into_string().map_err(Error::from) {
        Ok(config_dir) => config_dir,
        Err(error) => {
            on_init.call(error.to_error_code());
            return;
        }
    };

    // Get debug label
    let debug_label = match debug_label
        .map(CString::into_string)
        .transpose()
        .map_err(Error::from)
    {
        Ok(debug_label) => debug_label,
        Err(error) => {
            on_init.call(error.to_error_code());
            return;
        }
    };

    // Create tracing span
    let span = if let Some(debug_label) = debug_label {
        tracing::info_span!("service", message = debug_label)
    } else {
        Span::none()
    };

    // Setup the runtime
    let runtime = match runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(Error::InitializeRuntime)
    {
        Ok(runtime) => runtime,
        Err(error) => {
            on_init.call(error.to_error_code());
            return;
        }
    };

    let mut on_stop_rx = pin!(on_stop_rx);

    // Init the service. This can be cancelled with `service_stop`.
    let service = runtime.block_on(
        async {
            select! {
                result = Service::init(config_dir.into()) => result,
                result = &mut on_stop_rx => {
                    if let Ok(on_stop) = result {
                        on_stop.call(ErrorCode::Ok);
                    }

                    Err(Error::OperationInterrupted)
                }
            }
        }
        .instrument(span.clone()),
    );

    let mut service = match service {
        Ok(service) => {
            on_init.call(ErrorCode::Ok);
            service
        }
        Err(error) => {
            on_init.call(error.to_error_code());
            return;
        }
    };

    service.enable_panic_monitor();

    // Run the service until `service_stop` is called.
    runtime.block_on(
        async {
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

/// Initialize logging. Should be called before `service_start`.
#[unsafe(no_mangle)]
pub extern "C" fn init_log() {
    logger::init(LogFormat::Human, LogColor::Auto);
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

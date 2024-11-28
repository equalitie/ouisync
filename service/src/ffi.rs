use std::{
    ffi::{c_char, CStr},
    io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use ouisync_bridge::logger::{LogColor, LogFormat, Logger};
use tokio::runtime;

use crate::{
    protocol::{ErrorCode, LogLevel, ToErrorCode},
    Error, Service,
};

/// Start Ouisync service and bind it to the specified local socket. This function blocks until the
/// service is terminated by a client.
///
/// # Safety
///
/// `socket_path`, `config_dir` and `default_store_dir` must be safe to pass to
/// [std::ffi::CStr::from_ptr].
#[no_mangle]
pub unsafe extern "C" fn ouisync_start(
    socket_path: *const c_char,
    config_dir: *const c_char,
    default_store_dir: *const c_char,
) -> ErrorCode {
    start(socket_path, config_dir, default_store_dir).to_error_code()
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

unsafe fn start(
    socket_path: *const c_char,
    config_dir: *const c_char,
    default_store_dir: *const c_char,
) -> Result<(), Error> {
    let socket_path = PathBuf::from(CStr::from_ptr(socket_path).to_str()?);
    let config_dir = PathBuf::from(CStr::from_ptr(config_dir).to_str()?);
    let default_store_dir = PathBuf::from(CStr::from_ptr(default_store_dir).to_str()?);

    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(Error::InitializeRuntime)?;

    runtime.block_on(async move {
        let mut service = Service::init(socket_path, config_dir, default_store_dir).await?;
        service.run().await?;
        service.close().await;

        Ok(())
    })
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

unsafe fn log_init(log_file: *const c_char, log_tag: *const c_char) -> Result<(), Error> {
    let log_file = if log_file.is_null() {
        None
    } else {
        Some(Path::new(CStr::from_ptr(log_file).to_str()?))
    };

    let log_tag = CStr::from_ptr(log_tag).to_str()?.to_owned();

    let logger = Logger::new(log_file, log_tag, LogFormat::Human, LogColor::Auto)
        .map_err(Error::InitializeLogger)?;

    LOGGER.set(logger).map_err(|_| {
        Error::InitializeLogger(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "logger already initialized",
        ))
    })?;

    Ok(())
}

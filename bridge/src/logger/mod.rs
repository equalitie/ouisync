use crate::error::{Error, Result};
use file_rotate::{compression::Compression, suffix::AppendCount, ContentLimit, FileRotate};
use ouisync_lib::StateMonitor;
use std::{
    fs,
    panic::{self, PanicInfo},
    path::Path,
    sync::Mutex,
};
use tracing_subscriber::{
    fmt::{
        self,
        format::{Format, Pretty},
        Layer,
    },
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

pub struct Logger;

impl Logger {
    pub fn new(path: Option<&Path>, root_monitor: Option<StateMonitor>) -> Result<Self> {
        if let Some(parent) = path.and_then(|path| path.parent()) {
            fs::create_dir_all(parent).map_err(Error::InitializeLogger)?;
        }

        init_log(path);

        // Panic hook
        let default_panic_hook = panic::take_hook();

        if let Some(root_monitor) = root_monitor {
            let panic_counter = root_monitor
                .make_child("Session")
                .make_value("panic_counter", 0u32);

            panic::set_hook(Box::new(move |panic_info| {
                *panic_counter.get() += 1;
                log_panic(panic_info);
                default_panic_hook(panic_info);
            }));
        } else {
            panic::set_hook(Box::new(move |panic_info| {
                log_panic(panic_info);
                default_panic_hook(panic_info);
            }));
        }

        Ok(Self)
    }
}

#[cfg(not(target_os = "android"))]
fn init_log(log_path: Option<&Path>) {
    // Disable colors in output on Windows as `cmd` doesn't seem to support it.
    #[cfg(target_os = "windows")]
    let colors = false;
    #[cfg(not(target_os = "windows"))]
    let colors = true;

    // Log to stdout
    let stdout_layer = fmt::layer()
        .pretty()
        .with_ansi(colors)
        .with_target(false)
        .with_file(true)
        .with_line_number(true);

    // Log to file
    let file_layer = log_path.map(create_file_log_layer);

    tracing_subscriber::registry()
        .with(create_log_filter())
        .with(stdout_layer)
        .with(file_layer)
        .try_init()
        // `Err` here just means the logger is already initialized, it's OK to ignore it.
        .unwrap_or(());
}

#[cfg(target_os = "android")]
fn init_log(log_path: Option<&Path>) {
    use paranoid_android::{AndroidLogMakeWriter, Buffer};

    // Android log tag.
    // HACK: if the tag doesn't start with 'flutter' then the logs won't show up in the app if built in
    // release mode.
    const TAG: &str = "flutter-ouisync";

    let android_log_layer = fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .without_time()
        .with_writer(AndroidLogMakeWriter::with_buffer(
            TAG.to_owned(),
            Buffer::Main,
        )); // android log adds its own timestamp

    let file_layer = log_path.map(create_file_log_layer);

    tracing_subscriber::registry()
        .with(create_log_filter())
        .with(android_log_layer)
        .with(file_layer)
        .try_init()
        // `Err` here just means the logger is already initialized, it's OK to ignore it.
        .unwrap_or(());
}

fn create_log_filter() -> EnvFilter {
    EnvFilter::builder()
        // TODO: Allow changing the log level at runtime or at least at init
        // time (via a command-line option or so)
        .with_default_directive("ouisync=debug".parse().unwrap())
        .from_env_lossy()
}

fn create_file_log_layer<S>(
    path: &Path,
) -> Layer<S, Pretty, Format<Pretty>, Mutex<FileRotate<AppendCount>>> {
    let rotate = FileRotate::new(
        path,
        AppendCount::new(1),
        ContentLimit::BytesSurpassed(10 * 1024 * 1024),
        Compression::None,
        #[cfg(unix)]
        None,
    );

    fmt::layer()
        .pretty()
        .with_ansi(true)
        .with_target(false)
        .with_file(true)
        .with_line_number(true)
        .with_writer(Mutex::new(rotate))
}

fn log_panic(info: &PanicInfo) {
    match (info.payload().downcast_ref::<&str>(), info.location()) {
        (Some(message), Some(location)) => tracing::error!(
            "panic '{}' at {}:{}:{}",
            message,
            location.file(),
            location.line(),
            location.column(),
        ),
        (Some(message), None) => tracing::error!("panic '{message}'"),
        (None, Some(location)) => tracing::error!(
            "panic at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        (None, None) => tracing::error!("panic"),
    };
}

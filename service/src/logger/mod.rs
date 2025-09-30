mod color;
mod format;

#[cfg(target_os = "android")]
#[path = "android.rs"]
mod output;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
#[path = "stdout.rs"]
mod output;

// TODO: ios

pub use color::{LogColor, ParseLogColorError};
pub use format::{LogFormat, ParseLogFormatError};

use std::{
    env,
    panic::{self, PanicHookInfo},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Initialize logging. If the logger hasn't been initialized already, initializes it and returns
/// `true`. Otherwise does nothing and returns `false`.
pub fn init(format: LogFormat, color: LogColor) -> bool {
    let result = tracing_subscriber::registry()
        .with(create_log_filter())
        .with(output::layer(format, color))
        .try_init();

    // `Err` here means the logger is already initialized. Use it to ensure the panic hook is set up only once.
    if result.is_ok() {
        // Log panics
        let default_panic_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            log_panic(panic_info);
            default_panic_hook(panic_info);
        }));

        true
    } else {
        false
    }
}

fn create_log_filter() -> EnvFilter {
    EnvFilter::builder()
        // TODO: Allow changing the log level at runtime or at least at init
        // time (via a command-line option or so)
        .parse_lossy(
            env::var(EnvFilter::DEFAULT_ENV)
                .unwrap_or_else(|_| "ouisync=debug,state_monitor=warn,deadlock=warn".to_string()),
        )
}

fn log_panic(info: &PanicHookInfo) {
    match (
        info.payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str())),
        info.location(),
    ) {
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

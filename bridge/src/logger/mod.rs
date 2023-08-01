#[cfg(target_os = "android")]
mod android;

#[cfg(not(target_os = "android"))]
mod default;

mod common;

use crate::error::{Error, Result};
use ouisync_lib::StateMonitor;
use serde::{Deserialize, Serialize};
use std::{
    fmt, fs,
    panic::{self, PanicInfo},
    path::Path,
    str::FromStr,
};

#[cfg(target_os = "android")]
use self::android::Inner;

#[cfg(not(target_os = "android"))]
use self::default::Inner;

pub struct Logger {
    _inner: Inner,
}

impl Logger {
    pub fn new(
        path: Option<&Path>,
        root_monitor: Option<StateMonitor>,
        format: LogFormat,
    ) -> Result<Self> {
        if let Some(parent) = path.and_then(|path| path.parent()) {
            fs::create_dir_all(parent).map_err(Error::InitializeLogger)?;
        }

        let inner = Inner::new(path, format)?;

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

        Ok(Self { _inner: inner })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum LogFormat {
    /// human-readable
    Human,
    /// json (for machine processing)
    Json,
}

impl fmt::Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Human => write!(f, "human"),
            Self::Json => write!(f, "json"),
        }
    }
}

impl FromStr for LogFormat {
    type Err = ParseLogFormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "json" => Ok(Self::Json),
            _ => Err(ParseLogFormatError),
        }
    }
}

#[derive(Debug)]
pub struct ParseLogFormatError;

impl fmt::Display for ParseLogFormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid log format")
    }
}

impl std::error::Error for ParseLogFormatError {}

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

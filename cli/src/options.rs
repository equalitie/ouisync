use crate::APP_NAME;
use clap::{Parser, Subcommand};
use ouisync_bridge::logger::{LogColor, LogFormat};
use ouisync_service::protocol::Request;
use std::{env, path::PathBuf};

#[derive(Parser, Debug)]
#[command(name = APP_NAME, version, about)]
pub(crate) struct Options {
    /// Local socket (unix domain socket or windows named pipe) to connect to (if client) or to
    /// bind to (if server)
    ///
    /// Can be also specified with env variable OUISYNC_SOCKET.
    #[arg(short, long, default_value_os_t = default_socket(), value_name = "PATH")]
    pub socket: PathBuf,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Start the server
    Start {
        /// Config directory
        ///
        /// Can be also specified with env variable OUISYNC_CONFIG_DIR.
        #[arg(long, default_value_os_t = default_config_dir(), value_name = "PATH")]
        config_dir: PathBuf,

        /// Log format ("human" or "json")
        #[arg(long, default_value_t = LogFormat::Human)]
        log_format: LogFormat,

        /// When to color log messages ("always", "never" or "auto")
        ///
        /// "auto" means colors are used when printing directly to a terminal but not when
        /// redirected to a file or a pipe.
        #[arg(long, default_value_t)]
        log_color: LogColor,
    },
    #[command(flatten)]
    Request(Request),
}

/// Path to the config directory.
fn default_config_dir() -> PathBuf {
    env::var_os("OUISYNC_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .expect("config dir not defined")
                .join(APP_NAME)
        })
}

fn default_socket() -> PathBuf {
    env::var_os("OUISYNC_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(platform::default_socket)
}

mod platform {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn default_socket() -> PathBuf {
        // FIXME: when running as root, we should use `/run`
        dirs::runtime_dir()
            .or_else(dirs::cache_dir)
            .expect("neither runtime dir nor cache dir defined")
            .join(APP_NAME)
            .with_extension("sock")
    }

    #[cfg(target_os = "windows")]
    pub(super) fn default_socket() -> PathBuf {
        format!(r"\\.\pipe\{APP_NAME}").into()
    }
}

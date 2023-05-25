use crate::{protocol::Request, APP_NAME};
use clap::{Args, Parser};
use std::{env, path::PathBuf};

#[derive(Parser, Debug)]
#[command(name = APP_NAME, version, about)]
pub(crate) struct Options {
    #[command(flatten)]
    pub dirs: Dirs,

    /// Local socket (unix domain socket or windows named pipe) to connect to (if client) or to
    /// bind to (if server)
    ///
    /// Can be also specified with env variable OUISYNC_SOCKET.
    #[arg(short, long, default_value_os_t = default_socket(), value_name = "PATH")]
    pub socket: PathBuf,

    #[command(subcommand)]
    pub request: Request,
}

#[derive(Args, Debug)]
pub(crate) struct Dirs {
    /// Config directory
    ///
    /// Can be also specified with env variable OUISYNC_CONFIG_DIR.
    #[arg(long, default_value_os_t = default_config_dir(), value_name = "PATH")]
    pub config_dir: PathBuf,

    /// Repositories storage directory
    ///
    /// Can be also specified with env variable OUISYNC_STORE_DIR.
    #[arg(long, default_value_os_t = default_store_dir(), value_name = "PATH")]
    pub store_dir: PathBuf,

    /// Mount directory
    ///
    /// Can be also specified with env variable OUISYNC_MOUNT_DIR.
    #[arg(long, default_value_os_t = default_mount_dir(), value_name = "PATH")]
    pub mount_dir: PathBuf,
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

fn default_store_dir() -> PathBuf {
    env::var_os("OUISYNC_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("data dir not defined")
                .join(APP_NAME)
        })
}

fn default_mount_dir() -> PathBuf {
    env::var_os("OUISYNC_MOUNT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("home dir not defined")
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

    #[cfg(target_os = "linux")]
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

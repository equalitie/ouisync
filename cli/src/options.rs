use crate::{protocol::Request, APP_NAME};
use clap::{Args, Parser};
use once_cell::sync::Lazy;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = APP_NAME, version, about)]
pub(crate) struct Options {
    #[command(flatten)]
    pub dirs: Dirs,

    /// Local socket (unix domain socket or windows named pipe) to connect to (if client) or to
    /// bind to (if server)
    #[arg(short, long, default_value_t = default_socket(), value_name = "PATH")]
    pub socket: String,

    #[command(subcommand)]
    pub request: Request,
}

#[derive(Args, Debug)]
pub(crate) struct Dirs {
    /// Config directory
    #[arg(long, default_value = DEFAULT_CONFIG_DIR.as_os_str(), value_name = "PATH")]
    pub config_dir: PathBuf,

    /// Repositories storage directory
    #[arg(long, default_value = DEFAULT_DATA_DIR.as_os_str(), value_name = "PATH")]
    pub store_dir: PathBuf,

    /// Mount directory
    #[arg(long, default_value = DEFAULT_MOUNT_DIR.as_os_str(), value_name = "PATH")]
    pub mount_dir: PathBuf,
}

/// Path to the config directory.
static DEFAULT_CONFIG_DIR: Lazy<PathBuf> = Lazy::new(|| {
    dirs::config_dir()
        .expect("config dir not defined")
        .join(APP_NAME)
});

static DEFAULT_DATA_DIR: Lazy<PathBuf> = Lazy::new(|| {
    dirs::data_dir()
        .expect("data dir not defined")
        .join(APP_NAME)
});

static DEFAULT_MOUNT_DIR: Lazy<PathBuf> = Lazy::new(|| {
    dirs::home_dir()
        .expect("home dir not defined")
        .join(APP_NAME)
});

#[cfg(target_os = "linux")]
fn socket_dir() -> PathBuf {
    // FIXME: when running as root, we should use `/run`

    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .expect("neither runtime dir nor cache dir defined")
}

#[cfg(target_os = "linux")]
fn default_socket() -> String {
    socket_dir()
        .join(APP_NAME)
        .with_extension("sock")
        .into_os_string()
        .into_string()
        .expect("invalid utf8")
}

#[cfg(target_os = "windows")]
fn default_socket() -> String {
    format!(r"\\.\pipe\{APP_NAME}")
}

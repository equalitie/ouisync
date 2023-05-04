use crate::{protocol::Request, APP_NAME};
use camino::Utf8PathBuf;
use clap::{Args, Parser};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = APP_NAME, version, about)]
pub(crate) struct Options {
    #[command(flatten)]
    pub dirs: Dirs,

    /// Host socket to connect to (if client) or to bind to (if server)
    #[arg(short = 'H', long, default_value_t = default_host(), value_name = "ADDR")]
    pub host: String,

    #[command(subcommand)]
    pub request: Request,
}

#[derive(Args, Debug)]
pub(crate) struct Dirs {
    /// Config directory
    #[arg(long, default_value_t = default_config_dir(), value_name = "PATH")]
    pub config_dir: Utf8PathBuf,

    /// Repositories storage directory
    #[arg(long, default_value_t = default_data_dir(), value_name = "PATH")]
    pub store_dir: Utf8PathBuf,

    /// Mount directory
    #[arg(long, default_value_t = default_mount_dir(), value_name = "PATH")]
    pub mount_dir: Utf8PathBuf,
}

/// Path to the config directory.
fn default_config_dir() -> Utf8PathBuf {
    dirs::config_dir()
        .expect("config dir not defined")
        .join(APP_NAME)
        .try_into()
        .expect("invalid utf8 path")
}

fn default_data_dir() -> Utf8PathBuf {
    dirs::data_dir()
        .expect("data dir not defined")
        .join(APP_NAME)
        .try_into()
        .expect("invalid utf8 path")
}

fn default_mount_dir() -> Utf8PathBuf {
    dirs::home_dir()
        .expect("home dir not defined")
        .join(APP_NAME)
        .try_into()
        .expect("invalid utf8 path")
}

#[cfg(target_os = "linux")]
fn socket_dir() -> PathBuf {
    // FIXME: when running as root, we should use `/run`

    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .expect("neither runtime dir nor cache dir defined")
}

#[cfg(target_os = "linux")]
fn default_host() -> String {
    socket_dir()
        .join(APP_NAME)
        .with_extension("sock")
        .into_os_string()
        .into_string()
        .expect("invalid utf8")
}

#[cfg(target_os = "windows")]
fn default_host() -> String {
    format!(r"\\.\pipe\{APP_NAME}")
}

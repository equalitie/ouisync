use crate::APP_NAME;
use anyhow::{format_err, Error};
use std::{fmt, path::PathBuf, str::FromStr};

#[derive(Clone, Debug)]
pub enum HostAddr {
    Local(String),
    Remote(String),
}

impl FromStr for HostAddr {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if s.starts_with('/') || s.starts_with("\\\\") || s.starts_with('@') {
            return Ok(Self::Local(s.to_owned()));
        }

        if s.starts_with("ws://") || s.starts_with("wss://") {
            return Ok(Self::Remote(s.to_owned()));
        }

        Err(format_err!("invalid host socket").context(s.to_owned()))
    }
}

impl fmt::Display for HostAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(name) => write!(f, "{name}"),
            Self::Remote(url) => write!(f, "{url}"),
        }
    }
}

impl Default for HostAddr {
    fn default() -> Self {
        Self::Local(default_local())
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn default_local() -> String {
    socket_dir()
        .join(APP_NAME)
        .with_extension("sock")
        .into_os_string()
        .into_string()
        .expect("path not utf8")
}

#[cfg(target_os = "windows")]
pub(crate) fn default_local() -> String {
    format!(r"\\.\pipe\{APP_NAME}")
}

#[cfg(target_os = "linux")]
fn socket_dir() -> PathBuf {
    // FIXME: this panics when running as root. We should fall back to /run.
    dirs::runtime_dir().expect("runtime dir not defined")
}

// TODO: macos

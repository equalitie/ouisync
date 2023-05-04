use crate::APP_NAME;
use anyhow::{format_err, Error};
use std::{fmt, net::SocketAddr, path::PathBuf, str::FromStr};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostAddr {
    Local(PathBuf),
    Remote(Url),
}

impl FromStr for HostAddr {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First try to parse it as a ws:// or wss:// url.
        if let Ok(url) = Url::parse(s) {
            if url.scheme() == "ws" || url.scheme() == "wss" {
                return Ok(Self::Remote(url));
            } else {
                return Err(format_err!("unsupported url scheme '{}'", url.scheme()));
            }
        }

        // Then try to parse it as a raw socket address with implied 'ws://' scheme.
        if let Ok(addr) = SocketAddr::from_str(s) {
            return Ok(Self::Remote(Url::parse(&format!("ws://{addr}")).unwrap()));
        }

        // Otherwise assume local path but reject invalid windows named pipe paths
        let s = s.trim_start();
        if s.starts_with(r"\\") && !s.starts_with(r"\\.\pipe\") {
            return Err(format_err!("unsupported named pipe path '{}'", s));
        }

        Ok(Self::Local(PathBuf::from(s)))
    }
}

impl fmt::Display for HostAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(path) => write!(f, "{}", path.display()),
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
pub(crate) fn default_local() -> PathBuf {
    socket_dir().join(APP_NAME).with_extension("sock")
}

#[cfg(target_os = "windows")]
pub(crate) fn default_local() -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\{APP_NAME}"))
}

#[cfg(target_os = "linux")]
fn socket_dir() -> PathBuf {
    // FIXME: when running as root, we should use `/run`
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .expect("neither runtime dir nor cache dir defined")
}

// TODO: macos

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(
            HostAddr::from_str("/home/alice/ouisync.sock").unwrap(),
            HostAddr::Local(PathBuf::from("/home/alice/ouisync.sock"))
        );

        assert_eq!(
            HostAddr::from_str(r"\\.\pipe\ouisync").unwrap(),
            HostAddr::Local(PathBuf::from(r"\\.\pipe\ouisync")),
        );

        assert_eq!(
            HostAddr::from_str("ws://example.com/api").unwrap(),
            HostAddr::Remote(Url::parse("ws://example.com/api").unwrap())
        );

        assert_eq!(
            HostAddr::from_str("wss://example.com/api").unwrap(),
            HostAddr::Remote(Url::parse("wss://example.com/api").unwrap())
        );

        assert_eq!(
            HostAddr::from_str("192.168.1.40:54321").unwrap(),
            HostAddr::Remote(Url::parse("ws://192.168.1.40:54321").unwrap())
        );

        // Invalid scheme
        assert!(HostAddr::from_str("http://example.com/api").is_err());

        // Invalid windows named pipe paths
        assert!(HostAddr::from_str(r"\\.\invalid\ousiync").is_err());
        assert!(HostAddr::from_str(r"\\.\invalid\pipe\ousiync").is_err());
        assert!(HostAddr::from_str(r"\\remote\pipe\ousiync").is_err());
    }
}

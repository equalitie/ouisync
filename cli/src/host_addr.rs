use std::{convert::Infallible, fmt, net::SocketAddr, path::PathBuf, str::FromStr};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostAddr {
    Local(PathBuf),
    Remote(Url),
}

impl FromStr for HostAddr {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First try to parse it as url.
        if let Ok(url) = Url::parse(s) {
            return Ok(Self::Remote(url));
        }

        // Then try to parse it as a raw socket address with implied 'ws://' scheme.
        if let Ok(addr) = SocketAddr::from_str(s) {
            // unwrap ok because the url is valid
            return Ok(Self::Remote(Url::parse(&format!("ws://{addr}")).unwrap()));
        }

        // Otherwise assume local path
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

// TODO: macos

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(
            HostAddr::from_str("/home/alice/.cache/ouisync.sock").unwrap(),
            HostAddr::Local(PathBuf::from("/home/alice/.cache/ouisync.sock"))
        );

        assert_eq!(
            HostAddr::from_str("server/ouisync.sock").unwrap(),
            HostAddr::Local(PathBuf::from("server/ouisync.sock"))
        );

        assert_eq!(
            HostAddr::from_str(r"\\.\pipe\ouisync").unwrap(),
            HostAddr::Local(PathBuf::from(r"\\.\pipe\ouisync")),
        );

        assert_eq!(
            HostAddr::from_str(r"\\server\pipe\ouisync").unwrap(),
            HostAddr::Local(PathBuf::from(r"\\server\pipe\ouisync")),
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
            HostAddr::from_str("https://example.com/api").unwrap(),
            HostAddr::Remote(Url::parse("https://example.com/api").unwrap())
        );

        assert_eq!(
            HostAddr::from_str("192.168.1.40:54321").unwrap(),
            HostAddr::Remote(Url::parse("ws://192.168.1.40:54321").unwrap())
        );
    }
}

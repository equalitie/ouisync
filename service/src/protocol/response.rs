use crate::repository::RepositoryHandle;
use chrono::{DateTime, SecondsFormat, Utc};
use ouisync::{PeerAddr, PeerInfo, PeerSource, PeerState, StorageSize};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    None,
    Bool(bool),
    Expiration {
        block: Option<Duration>,
        repository: Option<Duration>,
    },
    Path(PathBuf),
    PeerAddrs(Vec<PeerAddr>),
    PeerInfo(Vec<PeerInfo>),
    QuotaInfo(QuotaInfo),
    Repository(RepositoryHandle),
    Repositories(BTreeMap<String, RepositoryHandle>),
    SocketAddrs(Vec<SocketAddr>),
    StorageSize(StorageSize),
    String(String),
    Strings(Vec<String>),
}

impl From<()> for Response {
    fn from(_: ()) -> Self {
        Self::None
    }
}

impl From<bool> for Response {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<PathBuf> for Response {
    fn from(value: PathBuf) -> Self {
        Self::Path(value)
    }
}

impl<'a> From<&'a Path> for Response {
    fn from(value: &'a Path) -> Self {
        Self::Path(value.to_owned())
    }
}

impl From<String> for Response {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl<'a> From<&'a str> for Response {
    fn from(value: &'a str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<Vec<String>> for Response {
    fn from(value: Vec<String>) -> Self {
        Self::Strings(value)
    }
}

impl From<RepositoryHandle> for Response {
    fn from(value: RepositoryHandle) -> Self {
        Self::Repository(value)
    }
}

impl From<BTreeMap<String, RepositoryHandle>> for Response {
    fn from(value: BTreeMap<String, RepositoryHandle>) -> Self {
        Self::Repositories(value)
    }
}

impl From<Vec<PeerInfo>> for Response {
    fn from(value: Vec<PeerInfo>) -> Self {
        Self::PeerInfo(value)
    }
}

impl From<Vec<PeerAddr>> for Response {
    fn from(value: Vec<PeerAddr>) -> Self {
        Self::PeerAddrs(value)
    }
}

impl From<Vec<SocketAddr>> for Response {
    fn from(value: Vec<SocketAddr>) -> Self {
        Self::SocketAddrs(value)
    }
}

impl From<StorageSize> for Response {
    fn from(value: StorageSize) -> Self {
        Self::StorageSize(value)
    }
}

impl From<QuotaInfo> for Response {
    fn from(value: QuotaInfo) -> Self {
        Self::QuotaInfo(value)
    }
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Bool(value) => write!(f, "{value}"),
            Self::String(value) => write!(f, "{value}"),
            Self::Strings(value) => {
                for item in value {
                    writeln!(f, "{item}")?;
                }

                Ok(())
            }
            Self::Repository(value) => write!(f, "{value}"),
            Self::Repositories(value) => {
                for (name, handle) in value {
                    writeln!(f, "{name}: {handle}")?;
                }

                Ok(())
            }
            Self::Path(value) => write!(f, "{}", value.display()),
            Self::PeerInfo(value) => {
                for peer in value {
                    writeln!(f, "{}", PeerInfoDisplay(peer))?;
                }

                Ok(())
            }
            Self::PeerAddrs(addrs) => {
                for addr in addrs {
                    writeln!(f, "{}", PeerAddrDisplay(addr))?;
                }

                Ok(())
            }
            Self::SocketAddrs(value) => {
                for addr in value {
                    writeln!(f, "{addr}")?;
                }

                Ok(())
            }
            Self::StorageSize(value) => write!(f, "{value}"),
            Self::QuotaInfo(info) => write!(f, "{info}"),
            Self::Expiration { block, repository } => write!(
                f,
                "block expiration: {block:?}, repository expiration: {repository:?}"
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QuotaInfo {
    pub quota: Option<StorageSize>,
    pub size: StorageSize,
}

impl fmt::Display for QuotaInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "quota:     ")?;

        if let Some(quota) = self.quota {
            writeln!(f, "{quota}")?;
        } else {
            writeln!(f, "∞")?;
        }

        write!(f, "available: ")?;

        if let Some(quota) = self.quota {
            let available = quota.saturating_sub(self.size);

            writeln!(
                f,
                "{} ({:.0}%)",
                available,
                percent(available.to_bytes(), quota.to_bytes())
            )?;
        } else {
            writeln!(f, "∞")?;
        }

        write!(f, "used:      {}", self.size)?;

        if let Some(quota) = self.quota {
            writeln!(
                f,
                " ({:.0}%)",
                percent(self.size.to_bytes(), quota.to_bytes())
            )?;
        } else {
            writeln!(f)?;
        }

        Ok(())
    }
}

fn percent(num: u64, den: u64) -> f64 {
    if den > 0 {
        100.0 * num as f64 / den as f64
    } else {
        0.0
    }
}

struct PeerAddrDisplay<'a>(&'a PeerAddr);

impl fmt::Display for PeerAddrDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            self.0.ip(),
            self.0.port(),
            match self.0 {
                PeerAddr::Tcp(_) => "tcp",
                PeerAddr::Quic(_) => "quic",
            },
        )
    }
}

struct PeerInfoDisplay<'a>(&'a PeerInfo);

impl fmt::Display for PeerInfoDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            PeerAddrDisplay(&self.0.addr),
            match self.0.source {
                PeerSource::UserProvided => "user-provided",
                PeerSource::Listener => "listener",
                PeerSource::LocalDiscovery => "local-discovery",
                PeerSource::Dht => "dht",
                PeerSource::PeerExchange => "pex",
            },
            match self.0.state {
                PeerState::Known => "known",
                PeerState::Connecting => "connecting",
                PeerState::Handshaking => "handshaking",
                PeerState::Active { .. } => "active",
            },
        )?;

        if let PeerState::Active { id, since } = &self.0.state {
            write!(
                f,
                " {} {} {} {}",
                id.as_public_key(),
                format_time(*since),
                self.0.stats.bytes_tx,
                self.0.stats.bytes_rx,
            )?;
        }

        Ok(())
    }
}

fn format_time(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ouisync::{PeerSource, PeerState, SecretRuntimeId, Stats};
    use rand::{rngs::StdRng, SeedableRng};
    use std::net::Ipv4Addr;

    #[test]
    fn peer_info_display() {
        let mut rng = StdRng::seed_from_u64(0);

        let addr: SocketAddr = (Ipv4Addr::LOCALHOST, 1248).into();
        let runtime_id = SecretRuntimeId::generate(&mut rng).public();

        assert_eq!(
            PeerInfoDisplay(&PeerInfo {
                addr: PeerAddr::Quic(addr),
                source: PeerSource::Dht,
                state: PeerState::Connecting,
                stats: Stats::default(),
            })
            .to_string(),
            "127.0.0.1 1248 quic dht connecting"
        );

        assert_eq!(
            PeerInfoDisplay(&PeerInfo {
                addr: PeerAddr::Quic(addr),
                source: PeerSource::Dht,
                state: PeerState::Active {
                    id: runtime_id,
                    since: DateTime::parse_from_rfc3339("2024-06-12T02:30:00Z")
                        .unwrap()
                        .into(),
                },
                stats: Stats {
                    bytes_tx: 1024,
                    bytes_rx: 4096,
                    throughput_tx: 0,
                    throughput_rx: 0,
                },
            })
            .to_string(),
            "127.0.0.1 \
             1248 \
             quic \
             dht \
             active \
             ee1aa49a4459dfe813a3cf6eb882041230c7b2558469de81f87c9bf23bf10a03 \
             2024-06-12T02:30:00Z \
             1024 \
             4096"
        );
    }
}

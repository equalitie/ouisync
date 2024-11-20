use chrono::{DateTime, SecondsFormat, Utc};
use ouisync::{PeerAddr, PeerInfo, PeerSource, PeerState};
use ouisync_service::protocol::{QuotaInfo, Response};
use std::{fmt, time::SystemTime};

pub(crate) fn print_response(response: &Response) {
    match response {
        Response::None => (),
        Response::Bool(value) => println!("{value}"),
        Response::String(value) => println!("{value}"),
        Response::Strings(value) => {
            for item in value {
                println!("{item}");
            }
        }
        Response::Repository(value) => println!("{value}"),
        Response::Repositories(value) => {
            for name in value.keys() {
                println!("{name}");
            }
        }
        Response::Path(value) => println!("{}", value.display()),
        Response::PeerInfo(value) => {
            for peer in value {
                println!("{}", PeerInfoDisplay(peer));
            }
        }
        Response::PeerAddrs(addrs) => {
            for addr in addrs {
                println!("{}", PeerAddrDisplay(addr));
            }
        }
        Response::SocketAddrs(value) => {
            for addr in value {
                println!("{addr}");
            }
        }
        Response::StorageSize(value) => println!("{value}"),
        Response::QuotaInfo(info) => println!("{}", QuotaInfoDisplay(info)),
        Response::Expiration { block, repository } => {
            println!("block expiration: {block:?}, repository expiration: {repository:?}")
        }
    }
}

struct QuotaInfoDisplay<'a>(&'a QuotaInfo);

impl fmt::Display for QuotaInfoDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "quota:     ")?;

        if let Some(quota) = self.0.quota {
            writeln!(f, "{quota}")?;
        } else {
            writeln!(f, "∞")?;
        }

        write!(f, "available: ")?;

        if let Some(quota) = self.0.quota {
            let available = quota.saturating_sub(self.0.size);

            writeln!(
                f,
                "{} ({:.0}%)",
                available,
                percent(available.to_bytes(), quota.to_bytes())
            )?;
        } else {
            writeln!(f, "∞")?;
        }

        write!(f, "used:      {}", self.0.size)?;

        if let Some(quota) = self.0.quota {
            writeln!(
                f,
                " ({:.0}%)",
                percent(self.0.size.to_bytes(), quota.to_bytes())
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
    use chrono::DateTime;
    use ouisync::{PeerSource, PeerState, SecretRuntimeId, Stats};
    use rand::{rngs::StdRng, SeedableRng};
    use std::net::{Ipv4Addr, SocketAddr};

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

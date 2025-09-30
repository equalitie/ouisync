use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

#[derive(Clone, Copy, Serialize, Deserialize, Debug)]
pub enum PeerPort {
    Tcp(u16),
    Quic(u16),
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum PeerAddr {
    Tcp(SocketAddr),
    Quic(SocketAddr),
}

impl PeerAddr {
    pub fn socket_addr(&self) -> &SocketAddr {
        match self {
            Self::Tcp(addr) => addr,
            Self::Quic(addr) => addr,
        }
    }

    pub fn ip(&self) -> IpAddr {
        self.socket_addr().ip()
    }

    pub fn port(&self) -> u16 {
        self.socket_addr().port()
    }

    pub fn set_port(&mut self, port: u16) {
        match self {
            Self::Tcp(addr) => addr.set_port(port),
            Self::Quic(addr) => addr.set_port(port),
        }
    }

    pub fn peer_port(&self) -> PeerPort {
        match self {
            Self::Tcp(addr) => PeerPort::Tcp(addr.port()),
            Self::Quic(addr) => PeerPort::Quic(addr.port()),
        }
    }

    pub fn is_quic(&self) -> bool {
        match self {
            Self::Tcp(_) => false,
            Self::Quic(_) => true,
        }
    }

    pub fn is_tcp(&self) -> bool {
        match self {
            Self::Tcp(_) => true,
            Self::Quic(_) => false,
        }
    }
}

impl FromStr for PeerAddr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (proto, addr) = match s.split_once('/') {
            Some((proto, addr)) => (proto, addr),
            None => {
                return Err(format!("Could not find '/' delimiter in the address {s:?}"));
            }
        };

        let addr = match SocketAddr::from_str(addr) {
            Ok(addr) => addr,
            Err(_) => return Err(format!("Failed to parse IP:PORT {addr:?}")),
        };

        if proto.eq_ignore_ascii_case("tcp") {
            Ok(PeerAddr::Tcp(addr))
        } else if proto.eq_ignore_ascii_case("quic") {
            Ok(PeerAddr::Quic(addr))
        } else {
            Err(format!("Unrecognized protocol {proto:?} in {s:?}"))
        }
    }
}

impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "tcp/{addr}"),
            Self::Quic(addr) => write!(f, "quic/{addr}"),
        }
    }
}

impl Serialize for PeerAddr {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if s.is_human_readable() {
            self.to_string().serialize(s)
        } else {
            SerdeProxy::serialize(self, s)
        }
    }
}

impl<'de> Deserialize<'de> for PeerAddr {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if d.is_human_readable() {
            <&str>::deserialize(d)?.parse().map_err(D::Error::custom)
        } else {
            SerdeProxy::deserialize(d)
        }
    }
}

// Proxy to serialize/deserialize PeerAddr in non human-readable formats.
#[derive(Serialize, Deserialize)]
#[serde(remote = "PeerAddr")]
enum SerdeProxy {
    Tcp(#[allow(dead_code)] SocketAddr),
    Quic(#[allow(dead_code)] SocketAddr),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parse() {
        for (orig, expected) in [
            (
                PeerAddr::Tcp((Ipv4Addr::UNSPECIFIED, 0).into()),
                "tcp/0.0.0.0:0",
            ),
            (
                PeerAddr::Tcp((Ipv6Addr::UNSPECIFIED, 0).into()),
                "tcp/[::]:0",
            ),
            (
                PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, 0).into()),
                "quic/0.0.0.0:0",
            ),
            (
                PeerAddr::Quic((Ipv6Addr::UNSPECIFIED, 0).into()),
                "quic/[::]:0",
            ),
        ] {
            assert_eq!(orig.to_string(), expected);
            assert_eq!(expected.parse::<PeerAddr>().unwrap(), orig);
        }
    }

    #[test]
    fn serialize_binary() {
        for (orig, expected) in [
            (
                PeerAddr::Tcp(([192, 0, 2, 0], 12481).into()),
                "0000000000000000c0000200c130",
            ),
            (
                PeerAddr::Quic(([0x2001, 0xdb8, 0x0, 0x1, 0x2, 0x3, 0x4, 0x5], 24816).into()),
                "010000000100000020010db8000000010002000300040005f060",
            ),
        ] {
            assert_eq!(hex::encode(bincode::serialize(&orig).unwrap()), expected);
            assert_eq!(
                bincode::deserialize::<PeerAddr>(&hex::decode(expected).unwrap()).unwrap(),
                orig
            );
        }
    }

    #[test]
    fn serialize_human_readable() {
        for addr in [
            PeerAddr::Tcp(([192, 0, 2, 0], 12481).into()),
            PeerAddr::Quic(([0x2001, 0xdb8, 0x0, 0x1, 0x2, 0x3, 0x4, 0x5], 24816).into()),
        ] {
            let expected = addr.to_string();
            let actual = serde_json::to_string(&addr).unwrap();
            assert_eq!(actual, format!("\"{expected}\""))
        }
    }
}

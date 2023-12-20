use serde::{Deserialize, Serialize};
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

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Serialize, Deserialize)]
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
                return Err(format!(
                    "Could not find '/' delimiter in the address {:?}",
                    s
                ));
            }
        };

        let addr = match SocketAddr::from_str(addr) {
            Ok(addr) => addr,
            Err(_) => return Err(format!("Failed to parse IP:PORT {:?}", addr)),
        };

        if proto.eq_ignore_ascii_case("tcp") {
            Ok(PeerAddr::Tcp(addr))
        } else if proto.eq_ignore_ascii_case("quic") {
            Ok(PeerAddr::Quic(addr))
        } else {
            Err(format!("Unrecognized protocol {:?} in {:?}", proto, s))
        }
    }
}

impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "tcp/{}", addr),
            Self::Quic(addr) => write!(f, "quic/{}", addr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parse() {
        assert_eq!(
            "tcp/0.0.0.0:0".parse::<PeerAddr>().unwrap(),
            PeerAddr::Tcp((Ipv4Addr::UNSPECIFIED, 0).into())
        );
        assert_eq!(
            "tcp/[::]:0".parse::<PeerAddr>().unwrap(),
            PeerAddr::Tcp((Ipv6Addr::UNSPECIFIED, 0).into())
        );

        assert_eq!(
            "quic/0.0.0.0:0".parse::<PeerAddr>().unwrap(),
            PeerAddr::Quic((Ipv4Addr::UNSPECIFIED, 0).into())
        );
        assert_eq!(
            "quic/[::]:0".parse::<PeerAddr>().unwrap(),
            PeerAddr::Quic((Ipv6Addr::UNSPECIFIED, 0).into())
        );
    }
}

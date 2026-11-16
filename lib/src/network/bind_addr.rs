use super::peer_addr::PeerAddr;
use std::{net::IpAddr, num::NonZeroU16};

/// Address to bind a network listener to.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct BindAddr {
    pub ip: BindIp,
    pub port: BindPort,
}

/// Protocol and IP address to bind a network listener to.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum BindIp {
    Tcp(IpAddr),
    Quic(IpAddr),
}

impl BindIp {
    pub fn ip(&self) -> IpAddr {
        match self {
            Self::Tcp(ip) | Self::Quic(ip) => *ip,
        }
    }
}

/// Port to bind a network listener to.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum BindPort {
    /// Bind to exactly this port. If it's taken, the listener is not bound.
    Strict(NonZeroU16),
    /// Try this port first and fall back to a random one if it's taken.
    Preferred(NonZeroU16),
    /// Bind to a random port.
    Random,
}

impl BindPort {
    /// The port to bind to first (`0` means random).
    pub(super) fn get(&self) -> u16 {
        match self {
            Self::Strict(port) | Self::Preferred(port) => port.get(),
            Self::Random => 0,
        }
    }
}

/// Port `0` is converted to [BindPort::Random], any other port to [BindPort::Strict].
impl From<PeerAddr> for BindAddr {
    fn from(addr: PeerAddr) -> Self {
        let ip = match addr {
            PeerAddr::Tcp(addr) => BindIp::Tcp(addr.ip()),
            PeerAddr::Quic(addr) => BindIp::Quic(addr.ip()),
        };

        let port = NonZeroU16::new(addr.port())
            .map(BindPort::Strict)
            .unwrap_or(BindPort::Random);

        Self { ip, port }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn from_peer_addr() {
        assert_eq!(
            BindAddr::from(PeerAddr::Quic((Ipv4Addr::LOCALHOST, 0).into())),
            BindAddr {
                ip: BindIp::Quic(Ipv4Addr::LOCALHOST.into()),
                port: BindPort::Random,
            }
        );

        assert_eq!(
            BindAddr::from(PeerAddr::Tcp((Ipv4Addr::UNSPECIFIED, 1234).into())),
            BindAddr {
                ip: BindIp::Tcp(Ipv4Addr::UNSPECIFIED.into()),
                port: BindPort::Strict(NonZeroU16::new(1234).unwrap()),
            }
        );
    }
}

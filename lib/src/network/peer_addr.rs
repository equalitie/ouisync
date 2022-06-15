use std::net::{IpAddr, SocketAddr};

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
}

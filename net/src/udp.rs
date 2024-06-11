pub use self::implementation::*;

use std::{
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr},
};

// Selected at random but to not clash with some reserved ones:
// https://www.iana.org/assignments/multicast-addresses/multicast-addresses.xhtml
pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 137);
pub const MULTICAST_PORT: u16 = 9271;

/// Trait for UDP-like sockets.
pub trait DatagramSocket {
    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        target: SocketAddr,
    ) -> impl Future<Output = io::Result<usize>> + Send + 'a;

    fn recv_from<'a>(
        &'a self,
        buf: &'a mut [u8],
    ) -> impl Future<Output = io::Result<(usize, SocketAddr)>> + Send + 'a;

    fn local_addr(&self) -> io::Result<SocketAddr>;
}

#[cfg(not(feature = "simulation"))]
mod implementation {
    use super::*;
    use crate::socket::{self, ReuseAddr};
    use std::net::SocketAddrV4;

    pub struct UdpSocket(tokio::net::UdpSocket);

    impl UdpSocket {
        /// Binds UDP socket to the given address. If the port is taken, uses a random one,
        pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
            Ok(Self(socket::bind(addr).await?))
        }

        pub async fn bind_multicast(interface: Ipv4Addr) -> io::Result<Self> {
            // Note: Don't bind to Ipv4Addr::UNSPECIFIED here, on some systems (or all, not sure)
            // the system would bind to the first network interface that "matches". If there are
            // two or more network interfaces the second call to this function would try to bind
            // to the same interface and we'd get the "Address already in use" error.
            let socket: tokio::net::UdpSocket = socket::bind_with_reuse_addr(
                SocketAddrV4::new(interface, MULTICAST_PORT).into(),
                ReuseAddr::Required,
            )
            .await?;

            socket.join_multicast_v4(MULTICAST_ADDR, interface)?;

            Ok(Self(socket))
        }

        pub fn into_std(self) -> io::Result<std::net::UdpSocket> {
            self.0.into_std()
        }
    }

    impl DatagramSocket for UdpSocket {
        async fn send_to<'a>(&'a self, buf: &'a [u8], target: SocketAddr) -> io::Result<usize> {
            self.0.send_to(buf, target).await
        }

        async fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> io::Result<(usize, SocketAddr)> {
            self.0.recv_from(buf).await
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            self.0.local_addr()
        }
    }
}

#[cfg(feature = "simulation")]
mod implementation {
    use super::*;

    pub struct UdpSocket;

    impl UdpSocket {
        pub async fn bind(_addr: SocketAddr) -> io::Result<Self> {
            unimplemented!("simulated udp sockets not supported")
        }

        pub async fn bind_multicast(_interface: Ipv4Addr) -> io::Result<Self> {
            unimplemented!("simulated udp sockets not supported")
        }

        pub fn into_std(self) -> io::Result<std::net::UdpSocket> {
            unimplemented!()
        }
    }

    impl DatagramSocket for UdpSocket {
        async fn send_to<'a>(&'a self, _buf: &'a [u8], _target: SocketAddr) -> io::Result<usize> {
            unimplemented!()
        }

        async fn recv_from<'a>(&'a self, _buf: &'a mut [u8]) -> io::Result<(usize, SocketAddr)> {
            unimplemented!()
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            unimplemented!()
        }
    }
}

use std::{io, net::SocketAddr};
use tokio::{
    net::{TcpListener, UdpSocket},
    task,
};

/// Binds socket to the given address. If the port is already taken, binds to a random available
/// port which can be retrieved by calling `local_addr` on the returned socket.
pub(super) async fn bind<T: Socket>(mut addr: SocketAddr) -> io::Result<T> {
    // Enable reuse address (`SO_REUSEADDR`) on the socket so that when the network or the whole
    // app is restarted, we can immediatelly re-bind to the same address as before.
    let socket: T = match bind_with_reuse_addr(addr, ReuseAddr::Preferred).await {
        Ok(socket) => Ok(socket),
        Err(e) => {
            // Try again with random port, unless we already used random port initially.
            if addr.port() != 0 {
                addr.set_port(0);
                bind_with_reuse_addr(addr, ReuseAddr::Preferred).await
            } else {
                Err(e)
            }
        }
    }?;

    Ok(socket)
}

pub(super) enum ReuseAddr {
    // Reuse address is required. If we fail to set it, we also fail to create the socket.
    Required,
    // Reuse address is a nice-to-have. If we fail to set it, we proceed with the socket creation
    // regardless.
    Preferred,
}

pub(super) async fn bind_with_reuse_addr<T: Socket>(
    addr: SocketAddr,
    reuse_addr: ReuseAddr,
) -> io::Result<T> {
    // Using socket2 because, std::net, nor async_std::net nor tokio::net lets
    // one set reuse_address(true) before "binding" the socket.
    let domain = socket2::Domain::for_address(addr);
    let socket = socket2::Socket::new(domain, T::RAW_TYPE, None)?;

    if let Err(error) = socket.set_reuse_address(true) {
        match reuse_addr {
            ReuseAddr::Required => return Err(error),
            ReuseAddr::Preferred => (),
        }
    }

    task::block_in_place(|| socket.bind(&addr.into()))?;

    socket.set_nonblocking(true)?;

    T::from_raw(socket)
}

// Internal trait to abstract over different types of network sockets.
pub(super) trait Socket: Sized {
    const RAW_TYPE: socket2::Type;

    fn from_raw(raw: socket2::Socket) -> io::Result<Self>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

impl Socket for TcpListener {
    const RAW_TYPE: socket2::Type = socket2::Type::STREAM;

    fn from_raw(raw: socket2::Socket) -> io::Result<Self> {
        // Marks the socket as ready for accepting incoming connections. This needs to be set for
        // TCP listeners otherwise we get "Invalid argument" error when calling `accept`.
        //
        // See https://stackoverflow.com/a/10002936/170073 for explanation of the parameter.
        raw.listen(128)?;

        TcpListener::from_std(raw.into())
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        TcpListener::local_addr(self)
    }
}

impl Socket for UdpSocket {
    const RAW_TYPE: socket2::Type = socket2::Type::DGRAM;

    fn from_raw(raw: socket2::Socket) -> io::Result<Self> {
        UdpSocket::from_std(raw.into())
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }
}

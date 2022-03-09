use crate::config::ConfigEntry;
use std::{future::Future, io, net::SocketAddr, pin::Pin};
use tokio::net::{TcpListener, UdpSocket};

/// Bind socket to the given address. If the port is 0, will try to use the same port as the last
/// time this function was called. The port is loaded/stored in the given config entry.
pub(super) async fn bind<T: Socket>(
    mut addr: SocketAddr,
    config: ConfigEntry<u16>,
) -> io::Result<T> {
    let original_port = addr.port();

    if addr.port() == 0 {
        if let Ok(last_port) = config.get().await {
            addr.set_port(last_port);
        }
    }

    let socket = match T::bind(addr).await {
        Ok(socket) => Ok(socket),
        Err(e) => {
            if original_port == 0 && original_port != addr.port() {
                addr.set_port(0);
                T::bind(addr).await
            } else {
                Err(e)
            }
        }
    }?;

    if let Ok(addr) = socket.local_addr() {
        // Ignore failures
        config.set(&addr.port()).await.ok();
    }

    Ok(socket)
}

// Internal trait to abstract over different types of network sockets.
pub(super) trait Socket: Sized {
    fn bind(addr: SocketAddr) -> DynIoFuture<Self>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

type DynIoFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>>>>;

impl Socket for TcpListener {
    fn bind(addr: SocketAddr) -> DynIoFuture<Self> {
        Box::pin(TcpListener::bind(addr))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        TcpListener::local_addr(self)
    }
}

impl Socket for UdpSocket {
    fn bind(addr: SocketAddr) -> DynIoFuture<Self> {
        Box::pin(UdpSocket::bind(addr))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        UdpSocket::local_addr(self)
    }
}
